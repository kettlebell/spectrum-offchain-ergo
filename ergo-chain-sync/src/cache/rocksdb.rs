use std::sync::Arc;

use async_trait::async_trait;
use ergo_lib::{
    chain::transaction::{Transaction, TxId},
    ergo_chain_types::BlockId,
    ergotree_ir::serialization::SigmaSerializable,
};
use rocksdb::WriteBatchWithTransaction;
use tokio::task::spawn_blocking;

use crate::model::{Block, BlockRecord};

use super::chain_cache::ChainCache;

static BEST_BLOCK: &str = "best_block";

/// Given a block `B`, let `HB` denote the (lowercase) hex-representation of block's ID. Then
///  - {HB}:p is the key which maps to the hex-representation of B's parent block ID.
///  - {HB}:c is the key which maps to the hex-representation of B's child block ID, if it currently
///    exists.
///  - {HB}:h is the key which maps to the height of `B`.
///  - {HB}:t is the key which maps to a binary-encoding of a Vec containing the hex-representation
///    `HT` of the transaction ID of every transaction of `B`.
///    - Every {HT} is a key which maps to the Ergo-binary-encoded representation of its
///      transaction.
///  - {BEST_BLOCK} is a key which maps to a Redis list of length 2, containing the
///    hex-representations of the best block's ID and its parent ID, in that order.
pub struct ChainCacheRocksDB {
    pub db: Arc<rocksdb::OptimisticTransactionDB>,
}

/// The Rocksdb bindings are not async, so we must wrap any uses of the library in
/// `tokio::task::spawn_blocking`.
#[async_trait(?Send)]
impl ChainCache for ChainCacheRocksDB {
    async fn append_block(&mut self, block: Block) {
        let db = self.db.clone();

        spawn_blocking(move || {
            let mut batch = WriteBatchWithTransaction::<true>::default();
            batch.put(
                &postfixed_key(&block.id, PARENT_POSTFIX),
                bincode::serialize(&block.parent_id).unwrap(),
            );
            batch.put(
                &postfixed_key(&block.id, HEIGHT_POSTFIX),
                bincode::serialize(&block.height).unwrap(),
            );

            let tx_ids: Vec<TxId> = block.transactions.iter().map(|t| t.id()).collect();
            // We package together all transactions ids into a Vec.
            batch.put(
                &postfixed_key(&block.id, TRANSACTION_POSTFIX),
                bincode::serialize(&tx_ids).unwrap(),
            );

            // Each transaction is stored in an Ergo-serialized binary representation.
            let serialized_transactions = block
                .transactions
                .iter()
                .map(|t| t.sigma_serialize_bytes().unwrap());

            // Map each transaction id to a bincode-encoded representation of its transaction.
            for (tx_id, tx) in tx_ids.into_iter().zip(serialized_transactions) {
                batch.put(bincode::serialize(&tx_id).unwrap(), tx);
            }

            batch.put(
                bincode::serialize(BEST_BLOCK).unwrap(),
                bincode::serialize(&BlockRecord {
                    id: block.id,
                    height: block.height,
                })
                .unwrap(),
            );

            assert!(db.write(batch).is_ok());
        })
        .await
        .unwrap();
    }

    async fn exists(&mut self, block_id: BlockId) -> bool {
        let db = self.db.clone();
        spawn_blocking(move || {
            db.get(postfixed_key(&block_id, HEIGHT_POSTFIX))
                .unwrap()
                .is_some()
        })
        .await
        .unwrap()
    }

    async fn get_best_block(&mut self) -> Option<BlockRecord> {
        let db = self.db.clone();
        spawn_blocking(move || {
            if let Ok(Some(bytes)) = db.get(bincode::serialize(BEST_BLOCK).unwrap()) {
                bincode::deserialize(&bytes).ok()
            } else {
                None
            }
        })
        .await
        .unwrap()
    }

    async fn take_best_block(&mut self) -> Option<Block> {
        let db = self.db.clone();
        spawn_blocking::<_, Option<Block>>(move || {
            let best_block_key = bincode::serialize(BEST_BLOCK).unwrap();

            loop {
                let db_tx = db.transaction();
                // The call to `get_for_update` is crucial; it plays an identical role as the WATCH
                // command in redis (refer to docs of `take_best_block` in impl of [`RedisClient`].
                if let Some(best_block_bytes) = db_tx.get_for_update(&best_block_key, true).unwrap() {
                    let BlockRecord { id, height } = bincode::deserialize(&best_block_bytes).unwrap();

                    if let Some(tx_ids_bytes) = db_tx.get(&postfixed_key(&id, TRANSACTION_POSTFIX)).unwrap() {
                        let mut transactions = vec![];
                        let tx_ids: Vec<TxId> = bincode::deserialize(&tx_ids_bytes).unwrap();
                        for tx_id in tx_ids {
                            let tx_key = bincode::serialize(&tx_id).unwrap();
                            let tx_bytes = db_tx.get(&tx_key).unwrap().unwrap();

                            // Don't need transaction anymore, delete
                            db_tx.delete(&tx_key).unwrap();

                            transactions.push(Transaction::sigma_parse_bytes(&tx_bytes).unwrap());
                        }

                        let parent_id_bytes =
                            db_tx.get(&postfixed_key(&id, PARENT_POSTFIX)).unwrap().unwrap();
                        let parent_id: BlockId = bincode::deserialize(&parent_id_bytes).unwrap();

                        db_tx.delete(&best_block_key).unwrap();

                        // The new best block will now be the parent of the old best block, if the parent
                        // exists in the cache.
                        if db_tx
                            .get(&postfixed_key(&parent_id, PARENT_POSTFIX))
                            .unwrap()
                            .is_some()
                        {
                            let parent_id_height_bytes = db_tx
                                .get(&postfixed_key(&parent_id, HEIGHT_POSTFIX))
                                .unwrap()
                                .unwrap();
                            let parent_id_height: u32 =
                                bincode::deserialize(&parent_id_height_bytes).unwrap();

                            db_tx
                                .put(
                                    &best_block_key,
                                    bincode::serialize(&BlockRecord {
                                        id: parent_id,
                                        height: parent_id_height,
                                    })
                                    .unwrap(),
                                )
                                .unwrap();
                        }
                        match db_tx.commit() {
                            Ok(_) => {
                                return Some(Block {
                                    id,
                                    parent_id,
                                    height,
                                    timestamp: 0, // todo: DEV-573
                                    transactions,
                                });
                            }
                            Err(e) => {
                                if e.kind() == rocksdb::ErrorKind::Busy {
                                    continue;
                                } else {
                                    panic!("Unexpected error: {}", e);
                                }
                            }
                        }
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
        })
        .await
        .unwrap()
    }
}

fn postfixed_key(block_id: &BlockId, s: &str) -> Vec<u8> {
    let mut bytes = bincode::serialize(block_id).unwrap();
    let p_bytes = bincode::serialize(s).unwrap();
    bytes.extend_from_slice(&p_bytes);
    bytes
}

const PARENT_POSTFIX: &str = ":p";
const HEIGHT_POSTFIX: &str = ":h";
const TRANSACTION_POSTFIX: &str = ":t";
