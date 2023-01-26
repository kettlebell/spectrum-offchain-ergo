use std::fmt::{Display, Formatter};

use derive_more::Display;
use ergo_lib::ergotree_ir::chain::ergo_box::{
    BoxTokens, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId, NonMandatoryRegisters,
};
use ergo_lib::ergotree_ir::chain::token::Token;
use ergo_lib::ergotree_ir::ergo_tree::{ErgoTree, ErgoTreeHeader};
use ergo_lib::ergotree_ir::mir::constant::{Constant, TryExtractInto};
use log::trace;
use nonempty::NonEmpty;
use serde::{Deserialize, Serialize};

use spectrum_offchain::data::OnChainEntity;
use spectrum_offchain::domain::TypedAssetAmount;
use spectrum_offchain::event_sink::handlers::types::{IntoBoxCandidate, TryFromBox};

use crate::data::assets::{Lq, PoolNft, Reward, Tmp, VirtLq};
use crate::data::bundle::{StakingBundle, StakingBundleProto, BUNDLE_KEY_AMOUNT_USER};
use crate::data::context::ExecutionContext;
use crate::data::executor::ExecutorOutput;
use crate::data::funding::{DistributionFunding, DistributionFundingProto};
use crate::data::miner::MinerOutput;
use crate::data::order::{Deposit, Redeem};
use crate::data::redeemer::{DepositOutput, RedeemOutput, RewardOutput};
use crate::data::{AsBox, PoolId, PoolStateId};
use crate::ergo::{NanoErg, DEFAULT_MINER_FEE, MIN_SAFE_BOX_VALUE, UNIT_VALUE};
use crate::validators::POOL_VALIDATOR;

pub const INIT_EPOCH_IX: u32 = 1;

#[derive(Debug, Eq, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub struct ProgramConfig {
    pub epoch_len: u32,
    pub epoch_num: u32,
    pub program_start: u32,
    pub redeem_blocks_delta: u32,
    pub max_rounding_error: u64,
    pub program_budget: TypedAssetAmount<Reward>,
}

impl From<ProgramConfig> for Vec<i32> {
    fn from(c: ProgramConfig) -> Self {
        vec![
            c.epoch_len as i32,
            c.epoch_num as i32,
            c.program_start as i32,
            c.redeem_blocks_delta as i32,
        ]
    }
}

#[derive(Debug, Display)]
pub enum PoolOperationError {
    Permanent(PermanentError),
    Temporal(TemporalError),
}

#[derive(Debug)]
pub enum PermanentError {
    LiqudityMismatch,
    ProgramExhausted,
    OrderPoisoned(String),
    LowValue { expected: u64, provided: u64 },
}

impl Display for PermanentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PermanentError::LiqudityMismatch => f.write_str("LiqudityMismatch"),
            PermanentError::ProgramExhausted => f.write_str("ProgramExhausted"),
            PermanentError::OrderPoisoned(detail) => f.write_str(&*format!("OrderPoisoned: {}", detail)),
            PermanentError::LowValue { expected, provided } => f.write_str(&*format!(
                "LowValue(expected: {}, provided: {})",
                expected, provided
            )),
        }
    }
}

#[derive(Debug, Display)]
pub enum TemporalError {
    LiquidityMoveBlocked,
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub pool_id: PoolId,
    pub budget_rem: TypedAssetAmount<Reward>,
    pub reserves_lq: TypedAssetAmount<Lq>,
    pub reserves_vlq: TypedAssetAmount<VirtLq>,
    pub reserves_tmp: TypedAssetAmount<Tmp>,
    pub epoch_ix: Option<u32>,
    pub conf: ProgramConfig,
    pub erg_value: NanoErg,
}

impl Display for Pool {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&*format!(
            "Pool[id={}, start={}, end={}, step={}]",
            self.pool_id,
            self.conf.program_start,
            self.program_end(),
            self.conf.epoch_len
        ))
    }
}

impl Display for AsBox<Pool> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&*format!(
            "Pool[id={}, state_id={}, start={}, end={}, step={}]",
            self.1.pool_id,
            PoolStateId::from(self.0.box_id()),
            self.1.conf.program_start,
            self.1.program_end(),
            self.1.conf.epoch_len
        ))
    }
}

impl Pool {
    pub fn pool_nft(&self) -> TypedAssetAmount<PoolNft> {
        TypedAssetAmount::new(self.pool_id.into(), UNIT_VALUE)
    }

    pub fn program_end(&self) -> u32 {
        self.conf.program_start + self.conf.epoch_len * self.conf.epoch_num
    }

    /// Apply deposit operation to the pool.
    /// Returns pool state after deposit, new staking bundle and user output in the case of success.
    /// Returns `PoolOperationError` otherwise.
    pub fn apply_deposit(
        self,
        deposit: Deposit,
        ctx: ExecutionContext,
    ) -> Result<
        (
            Pool,
            StakingBundleProto,
            DepositOutput,
            ExecutorOutput,
            MinerOutput,
        ),
        PoolOperationError,
    > {
        if self.num_epochs_remain(ctx.height) < 1 {
            return Err(PoolOperationError::Permanent(PermanentError::ProgramExhausted));
        }
        if !self.epoch_completed() {
            return Err(PoolOperationError::Temporal(TemporalError::LiquidityMoveBlocked));
        }
        let epochs_remain = self.num_epochs_remain(ctx.height);
        if epochs_remain != deposit.expected_num_epochs {
            return Err(PoolOperationError::Permanent(PermanentError::OrderPoisoned(
                format!(
                    "Expected epochs [{}], remain epochs [{}]",
                    deposit.expected_num_epochs, epochs_remain
                ),
            )));
        }
        let release_vlq = TypedAssetAmount::new(self.reserves_vlq.token_id, deposit.lq.amount);
        let release_tmp_amt = epochs_remain as u64 * release_vlq.amount;
        let release_tmp = TypedAssetAmount::new(self.reserves_tmp.token_id, release_tmp_amt);
        let mut next_pool = self;
        next_pool.reserves_lq = next_pool.reserves_lq + deposit.lq;
        next_pool.reserves_vlq = next_pool.reserves_vlq - release_vlq;
        next_pool.reserves_tmp = next_pool.reserves_tmp - release_tmp;
        let bundle_key_for_user = TypedAssetAmount::new(ctx.mintable_token_id, BUNDLE_KEY_AMOUNT_USER);
        let bundle = StakingBundleProto {
            bundle_key_id: bundle_key_for_user.to_asset(),
            pool_id: next_pool.pool_id,
            vlq: release_vlq,
            tmp: release_tmp,
            redeemer_prop: deposit.redeemer_prop.clone(),
            erg_value: MIN_SAFE_BOX_VALUE,
        };
        let user_output = DepositOutput {
            bundle_key: bundle_key_for_user,
            redeemer_prop: deposit.redeemer_prop,
            erg_value: MIN_SAFE_BOX_VALUE,
        };
        let miner_output = MinerOutput {
            erg_value: DEFAULT_MINER_FEE,
        };
        let remainder_erg = deposit
            .erg_value
            .safe_sub(bundle.erg_value)
            .safe_sub(user_output.erg_value)
            .safe_sub(miner_output.erg_value);
        if remainder_erg >= MIN_SAFE_BOX_VALUE {
            let executor_output = ExecutorOutput {
                executor_prop: ctx.executor_prop,
                erg_value: remainder_erg,
            };
            Ok((next_pool, bundle, user_output, executor_output, miner_output))
        } else {
            Err(PoolOperationError::Permanent(PermanentError::LowValue {
                expected: MIN_SAFE_BOX_VALUE.into(),
                provided: remainder_erg.into(),
            }))
        }
    }

    /// Apply redeem operation to the pool.
    /// Returns pool state after redeem and user output in the case of success.
    /// Returns `PoolOperationError` otherwise.
    pub fn apply_redeem(
        self,
        redeem: Redeem,
        bundle: StakingBundle,
        ctx: ExecutionContext,
    ) -> Result<(Pool, RedeemOutput, ExecutorOutput, MinerOutput), PoolOperationError> {
        if redeem.expected_lq.amount != bundle.vlq.amount {
            return Err(PoolOperationError::Permanent(PermanentError::LiqudityMismatch));
        }
        if !self.epoch_completed() {
            return Err(PoolOperationError::Temporal(TemporalError::LiquidityMoveBlocked));
        }
        let release_lq = redeem.expected_lq;
        let mut next_pool = self;
        next_pool.reserves_lq = next_pool.reserves_lq - release_lq;
        next_pool.reserves_vlq = next_pool.reserves_vlq + bundle.vlq;
        next_pool.reserves_tmp = next_pool.reserves_tmp + bundle.tmp;
        let user_output = RedeemOutput {
            lq: release_lq,
            redeemer_prop: redeem.redeemer_prop,
            erg_value: MIN_SAFE_BOX_VALUE,
        };
        let miner_output = MinerOutput {
            erg_value: DEFAULT_MINER_FEE,
        };
        let remainder_erg = (redeem.erg_value + bundle.erg_value)
            .safe_sub(user_output.erg_value)
            .safe_sub(miner_output.erg_value);
        if remainder_erg >= MIN_SAFE_BOX_VALUE {
            let executor_output = ExecutorOutput {
                executor_prop: ctx.executor_prop,
                erg_value: remainder_erg,
            };
            Ok((next_pool, user_output, executor_output, miner_output))
        } else {
            Err(PoolOperationError::Permanent(PermanentError::LowValue {
                expected: MIN_SAFE_BOX_VALUE.into(),
                provided: remainder_erg.into(),
            }))
        }
    }

    /// Distribute rewards in batch.
    /// Returns state of the pool after distribution, subtracted bundlesa and reward outputs.
    pub fn distribute_rewards(
        self,
        bundles: Vec<StakingBundle>,
        funding: NonEmpty<DistributionFunding>,
    ) -> Result<
        (
            Pool,
            Vec<StakingBundle>,
            Option<DistributionFundingProto>,
            Vec<RewardOutput>,
            MinerOutput,
        ),
        PoolOperationError,
    > {
        let epoch_alloc = self.epoch_alloc();
        let epoch_ix = if self.epoch_completed() {
            ((self.conf.epoch_num + 1) as u64 - (self.budget_rem.amount / epoch_alloc)) as u32
        } else {
            ((self.conf.epoch_num + 1) as u64 - (self.budget_rem.amount / epoch_alloc) - 1) as u32
        };
        let epochs_remain = self.conf.epoch_num - epoch_ix;
        let lq_reserves_0 = self.reserves_lq;
        let mut next_pool = self;
        let mut next_bundles = bundles;
        let mut reward_outputs = Vec::new();
        let mut accumulated_cost = NanoErg::from(0u64);
        for mut bundle in &mut next_bundles {
            let epochs_burned = (bundle.tmp.amount / bundle.vlq.amount).saturating_sub(epochs_remain as u64);
            trace!(target: "pool", "Bundle: [{}], epochs_remain: [{}], epochs_burned: [{}]", bundle.state_id, epochs_remain, epochs_burned);
            if epochs_burned < 1 {
                return Err(PoolOperationError::Permanent(PermanentError::OrderPoisoned(
                    format!("Already compounded"),
                )));
            }
            let reward_amt =
                (next_pool.epoch_alloc() as u128 * bundle.vlq.amount as u128 * epochs_burned as u128
                    / (lq_reserves_0.amount - 1) as u128) as u64;
            let reward = TypedAssetAmount::new(next_pool.budget_rem.token_id, reward_amt);
            let reward_output = RewardOutput {
                reward,
                redeemer_prop: bundle.redeemer_prop.clone(),
                erg_value: MIN_SAFE_BOX_VALUE,
            };
            let charged_tmp_amt = bundle.tmp.amount - epochs_remain as u64 * bundle.vlq.amount;
            let charged_tmp = TypedAssetAmount::new(bundle.tmp.token_id, charged_tmp_amt);
            accumulated_cost = accumulated_cost + reward_output.erg_value;
            reward_outputs.push(reward_output);
            bundle.tmp = bundle.tmp - charged_tmp;
            next_pool.reserves_tmp = next_pool.reserves_tmp + charged_tmp;
            next_pool.budget_rem = next_pool.budget_rem - reward;
        }
        next_pool.epoch_ix = Some(epoch_ix);
        let mut miner_output = MinerOutput {
            erg_value: DEFAULT_MINER_FEE,
        };
        accumulated_cost = accumulated_cost + miner_output.erg_value;
        let funds_total: NanoErg = funding.iter().map(|f| f.erg_value).sum();
        let funds_remain = funds_total.safe_sub(accumulated_cost);
        let next_funding_box = if funds_remain >= MIN_SAFE_BOX_VALUE {
            Some(DistributionFundingProto {
                prop: funding.head.prop,
                erg_value: funds_remain,
            })
        } else {
            miner_output.erg_value = miner_output.erg_value + funds_remain;
            None
        };
        Ok((
            next_pool,
            next_bundles,
            next_funding_box,
            reward_outputs,
            miner_output,
        ))
    }

    pub fn epochs_left_to_process(&self) -> u32 {
        println!("Epoch Alloc: {}", self.epoch_alloc());
        (self
            .budget_rem
            .amount
            .saturating_sub(self.conf.max_rounding_error) as f64
            / self.epoch_alloc() as f64)
            .ceil() as u32
    }

    fn epoch_alloc(&self) -> u64 {
        self.conf.program_budget.amount / self.conf.epoch_num as u64
    }

    fn num_epochs_remain(&self, height: u32) -> u32 {
        self.conf.epoch_num.saturating_sub(self.current_epoch_ix(height))
    }

    fn current_epoch_ix(&self, height: u32) -> u32 {
        let cur_block_ix = height as i64 - self.conf.program_start as i64 + 1;
        let cur_epoch_ix_rem = cur_block_ix % self.conf.epoch_len as i64;
        let cur_epoch_ix_r = cur_block_ix / self.conf.epoch_len as i64;
        if cur_epoch_ix_rem == 0 && cur_epoch_ix_r == 0 {
            0
        } else {
            if cur_epoch_ix_rem > 0 {
                cur_epoch_ix_r as u32 + 1
            } else {
                cur_epoch_ix_r as u32
            }
        }
    }

    fn epoch_completed(&self) -> bool {
        self.budget_rem.amount as f64 % self.epoch_alloc() as f64 <= self.conf.max_rounding_error as f64
    }
}

impl OnChainEntity for AsBox<Pool> {
    type TEntityId = PoolId;
    type TStateId = PoolStateId;

    fn get_self_ref(&self) -> Self::TEntityId {
        self.1.pool_id
    }

    fn get_self_state_ref(&self) -> Self::TStateId {
        self.0.box_id().into()
    }
}

impl TryFromBox for Pool {
    fn try_from_box(bx: ErgoBox) -> Option<Pool> {
        let r4 = bx.get_register(NonMandatoryRegisterId::R4.into());
        let r5 = bx.get_register(NonMandatoryRegisterId::R5.into());
        let r6 = bx.get_register(NonMandatoryRegisterId::R6.into());
        let r7 = bx.get_register(NonMandatoryRegisterId::R7.into());
        if let Some(tokens) = &bx.tokens {
            if tokens.len() == 5 && *POOL_VALIDATOR == bx.ergo_tree {
                let pool_nft = tokens.get(0).unwrap().token_id;
                let budget_rem = tokens.get(1)?;
                let lq = tokens.get(2)?;
                let vlq = tokens.get(3)?;
                let tmp = tokens.get(4)?;
                let conf = r4?.v.try_extract_into::<Vec<i32>>().ok()?;
                let budget = r5?.v.try_extract_into::<i64>().ok()?;
                let max_rounding_error = <u64>::try_from(r6?.v.try_extract_into::<i64>().ok()?).ok()?;
                let epoch_ix = r7
                    .and_then(|reg| reg.v.try_extract_into::<i32>().ok())
                    .and_then(|x| <u32>::try_from(x).ok());
                let conf = ProgramConfig {
                    epoch_len: *conf.get(0)? as u32,
                    epoch_num: *conf.get(1)? as u32,
                    program_start: *conf.get(2)? as u32,
                    redeem_blocks_delta: *conf.get(3)? as u32,
                    max_rounding_error,
                    program_budget: TypedAssetAmount::new(budget_rem.token_id, budget as u64),
                };
                return Some(Pool {
                    pool_id: PoolId::from(pool_nft),
                    budget_rem: TypedAssetAmount::new(budget_rem.token_id, *budget_rem.amount.as_u64()),
                    reserves_lq: TypedAssetAmount::new(lq.token_id, *lq.amount.as_u64()),
                    reserves_vlq: TypedAssetAmount::new(vlq.token_id, *vlq.amount.as_u64()),
                    reserves_tmp: TypedAssetAmount::new(tmp.token_id, *tmp.amount.as_u64()),
                    epoch_ix,
                    conf,
                    erg_value: bx.value.into(),
                });
            }
        }
        None
    }
}

impl IntoBoxCandidate for Pool {
    fn into_candidate(self, height: u32) -> ErgoBoxCandidate {
        let tokens = BoxTokens::from_vec(vec![
            Token::from(self.pool_nft()),
            Token::from(self.budget_rem),
            Token::from(self.reserves_lq),
            Token::from(self.reserves_vlq),
            Token::from(self.reserves_tmp),
        ])
        .unwrap();
        let registers = NonMandatoryRegisters::try_from(
            vec![
                Constant::from(<Vec<i32>>::from(self.conf)),
                Constant::from(self.conf.program_budget.amount as i64),
                Constant::from(self.conf.max_rounding_error as i64),
            ]
            .into_iter()
            .chain(
                self.epoch_ix
                    .map(|ix| vec![Constant::from(ix as i32)])
                    .unwrap_or(Vec::new())
                    .into_iter(),
            )
            .collect::<Vec<_>>(),
        )
        .unwrap();
        ErgoBoxCandidate {
            value: self.erg_value.into(),
            ergo_tree: POOL_VALIDATOR.clone(),
            tokens: Some(tokens),
            additional_registers: registers,
            creation_height: height,
        }
    }
}

#[cfg(test)]
mod tests {
    use ergo_lib::ergo_chain_types::{blake2b256_hash, Digest32};
    use ergo_lib::ergotree_ir::chain::ergo_box::{BoxId, ErgoBox};
    use ergo_lib::ergotree_ir::chain::token::TokenId;
    use ergo_lib::ergotree_ir::ergo_tree::ErgoTree;
    use ergo_lib::ergotree_ir::mir::constant::Constant;
    use ergo_lib::ergotree_ir::mir::expr::Expr;
    use ergo_lib::ergotree_ir::serialization::SigmaSerializable;
    use ergo_lib::ergotree_ir::sigma_protocol::sigma_boolean::{ProveDlog, SigmaProp};
    use nonempty::nonempty;
    use rand::Rng;

    use spectrum_offchain::domain::TypedAssetAmount;
    use spectrum_offchain::event_sink::handlers::types::TryFromBox;

    use crate::data::bundle::StakingBundle;
    use crate::data::context::ExecutionContext;
    use crate::data::funding::DistributionFunding;
    use crate::data::order::{Deposit, Redeem};
    use crate::data::pool::{Pool, ProgramConfig};
    use crate::data::{BundleStateId, FundingId, OrderId, PoolId};
    use crate::ergo::{NanoErg, MAX_VALUE};
    use crate::validators::BUNDLE_VALIDATOR;

    fn make_pool(epoch_len: u32, epoch_num: u32, program_start: u32, program_budget: u64) -> Pool {
        Pool {
            pool_id: PoolId::from(TokenId::from(random_digest())),
            budget_rem: TypedAssetAmount::new(TokenId::from(random_digest()), program_budget),
            reserves_lq: TypedAssetAmount::new(TokenId::from(random_digest()), 0),
            reserves_vlq: TypedAssetAmount::new(TokenId::from(random_digest()), MAX_VALUE),
            reserves_tmp: TypedAssetAmount::new(TokenId::from(random_digest()), MAX_VALUE),
            epoch_ix: None,
            conf: ProgramConfig {
                epoch_len,
                epoch_num,
                program_start,
                redeem_blocks_delta: 0,
                max_rounding_error: 1,
                program_budget: TypedAssetAmount::new(TokenId::from(random_digest()), program_budget),
            },
            erg_value: NanoErg::from(100000000000u64),
        }
    }

    fn random_digest() -> Digest32 {
        let mut rng = rand::thread_rng();
        Digest32::try_from(
            (0..32)
                .map(|_| (rng.gen_range(0..=256) % 256) as u8)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn trivial_prop() -> ErgoTree {
        ErgoTree::try_from(Expr::Const(Constant::from(true))).unwrap()
    }

    fn trivial_sigma_prop() -> SigmaProp {
        let sample = "0008cd03171b64b4b185c2581d421ae0ec1f4ef2a60cf849b0f51de99f97e4c89f2500e3";
        SigmaProp::from(
            ProveDlog::try_from(ErgoTree::sigma_parse_bytes(&*base16::decode(sample).unwrap()).unwrap())
                .unwrap(),
        )
    }

    #[test]
    fn early_deposit() {
        let budget = 1000000000;
        let pool = make_pool(10, 10, 10, budget);
        let deposit_lq = TypedAssetAmount::new(pool.reserves_lq.token_id, 1000);
        let deposit = Deposit {
            order_id: OrderId::from(BoxId::from(random_digest())),
            pool_id: pool.pool_id,
            redeemer_prop: trivial_sigma_prop(),
            lq: deposit_lq,
            erg_value: NanoErg::from(100000000000u64),
            expected_num_epochs: 10,
            bundle_prop_hash: make_staking_bundle_prop_hash(),
            max_miner_fee: 10000000,
        };
        let ctx = ExecutionContext {
            height: 9,
            mintable_token_id: TokenId::from(random_digest()),
            executor_prop: trivial_prop(),
        };
        let (pool2, bundle, _output, rew, _) = pool.clone().apply_deposit(deposit.clone(), ctx).unwrap();
        assert_eq!(bundle.vlq.amount, deposit.lq.amount);
        assert_eq!(bundle.tmp.amount, pool.conf.epoch_num as u64 * deposit_lq.amount);
        assert_eq!(pool2.reserves_lq, deposit.lq);
        assert_eq!(pool2.reserves_lq - pool.reserves_lq, deposit.lq);
        assert_eq!(pool2.reserves_vlq, pool.reserves_vlq - bundle.vlq);
        assert_eq!(pool2.reserves_tmp, pool.reserves_tmp - bundle.tmp);
    }

    #[test]
    fn deposit_redeem() {
        let budget = 1000000000;
        let pool = make_pool(10, 10, 10, budget);
        let deposit = TypedAssetAmount::new(pool.reserves_lq.token_id, 1000);
        let deposit = Deposit {
            order_id: OrderId::from(BoxId::from(random_digest())),
            pool_id: pool.pool_id,
            redeemer_prop: trivial_sigma_prop(),
            lq: deposit,
            erg_value: NanoErg::from(100000000000u64),
            expected_num_epochs: 9,
            bundle_prop_hash: make_staking_bundle_prop_hash(),
            max_miner_fee: 10000000,
        };
        let ctx = ExecutionContext {
            height: 10,
            mintable_token_id: TokenId::from(random_digest()),
            executor_prop: trivial_prop(),
        };
        let (pool2, bundle, output, rew, _) =
            pool.clone().apply_deposit(deposit.clone(), ctx.clone()).unwrap();
        let redeem = Redeem {
            order_id: OrderId::from(BoxId::from(random_digest())),
            pool_id: pool.pool_id,
            redeemer_prop: trivial_prop(),
            bundle_key: output.bundle_key,
            expected_lq: deposit.lq,
            erg_value: NanoErg::from(100000000000u64),
            max_miner_fee: 10000000,
        };
        let (pool3, output, rew, _) = pool2
            .clone()
            .apply_redeem(
                redeem.clone(),
                StakingBundle::from_proto(bundle.clone(), BundleStateId::from(BoxId::from(random_digest()))),
                ctx,
            )
            .unwrap();

        assert_eq!(pool.reserves_lq, pool3.reserves_lq);
        assert_eq!(pool.reserves_vlq, pool3.reserves_vlq);
        assert_eq!(pool.reserves_tmp, pool3.reserves_tmp);
        assert_eq!(output.lq, deposit.lq);
    }

    fn make_staking_bundle_prop_hash() -> Digest32 {
        blake2b256_hash(&BUNDLE_VALIDATOR.sigma_serialize_bytes().unwrap())
    }

    #[test]
    fn distribute_rewards() {
        let budget = 1000000000;
        let pool = make_pool(10, 10, 10, budget);
        let deposit_amt_a = TypedAssetAmount::new(pool.reserves_lq.token_id, 1000);
        let deposit_a = Deposit {
            order_id: OrderId::from(BoxId::from(random_digest())),
            pool_id: pool.pool_id,
            redeemer_prop: trivial_sigma_prop(),
            lq: deposit_amt_a,
            erg_value: NanoErg::from(100000000000u64),
            expected_num_epochs: 10,
            bundle_prop_hash: make_staking_bundle_prop_hash(),
            max_miner_fee: 10000000,
        };
        let deposit_disproportion = 2;
        let deposit_amt_b = TypedAssetAmount::new(
            pool.reserves_lq.token_id,
            deposit_amt_a.amount * deposit_disproportion,
        );
        let deposit_b = Deposit {
            order_id: OrderId::from(BoxId::from(random_digest())),
            pool_id: pool.pool_id,
            redeemer_prop: trivial_sigma_prop(),
            lq: deposit_amt_b,
            erg_value: NanoErg::from(100000000000u64),
            expected_num_epochs: 10,
            bundle_prop_hash: make_staking_bundle_prop_hash(),
            max_miner_fee: 10000000,
        };
        let ctx_1 = ExecutionContext {
            height: 9,
            mintable_token_id: TokenId::from(random_digest()),
            executor_prop: trivial_prop(),
        };
        let (pool_2, bundle_a, _output_a, rew, _) = pool
            .clone()
            .apply_deposit(deposit_a.clone(), ctx_1.clone())
            .unwrap();
        let (pool_3, bundle_b, _output_b, rew, _) =
            pool_2.clone().apply_deposit(deposit_b.clone(), ctx_1).unwrap();
        let funding = nonempty![DistributionFunding {
            id: FundingId::from(BoxId::from(random_digest())),
            prop: trivial_prop(),
            erg_value: NanoErg::from(2000000000u64),
        }];
        let (_pool_4, bundles, next_funding, rewards, _) = pool_3
            .distribute_rewards(
                Vec::from([
                    StakingBundle::from_proto(
                        bundle_a.clone(),
                        BundleStateId::from(BoxId::from(random_digest())),
                    ),
                    StakingBundle::from_proto(
                        bundle_b.clone(),
                        BundleStateId::from(BoxId::from(random_digest())),
                    ),
                ]),
                funding,
            )
            .unwrap();
        assert_eq!(
            rewards[0].reward.amount * deposit_disproportion,
            rewards[1].reward.amount
        );
        assert_eq!(bundles[0].tmp, bundle_a.tmp - bundle_a.vlq.coerce());
        assert_eq!(bundles[1].tmp, bundle_b.tmp - bundle_b.vlq.coerce());
    }

    #[test]
    fn parse_pool() {
        let bx: ErgoBox = serde_json::from_str(POOL_JSON).unwrap();
        let res = Pool::try_from_box(bx);
        assert!(res.is_some())
    }

    const POOL_JSON: &str = r#"{
        "boxId": "7153d14ec1fad42943102cc0541c96e44549f5c4e496e27f43b0885fd4a9fd43",
        "value": 1250000,
        "ergoTree": "19ec052404000400040204020404040404060406040804080404040204000400040204020400040a050005000404040204020e2074aeba0675c10c7fff46d3aa5e5a8efc55f0b0d87393dcb2f4b0a04be213cecb040004020500040204020406050005000402050205000500d81ed601b2a5730000d602db63087201d603db6308a7d604b27203730100d605e4c6a70410d606e4c6a70505d607e4c6a70605d608b27202730200d609b27203730300d60ab27202730400d60bb27203730500d60cb27202730600d60db27203730700d60e8c720d01d60fb27202730800d610b27203730900d6118c721001d6128c720b02d613998c720a027212d6148c720902d615b27205730a00d6169a99a37215730bd617b27205730c00d6189d72167217d61995919e72167217730d9a7218730e7218d61ab27205730f00d61b7e721a05d61c9d7206721bd61d998c720c028c720d02d61e998c720f028c721002d1ededededed93b272027310007204ededed93e4c672010410720593e4c672010505720693e4c6720106057207928cc77201018cc7a70193c27201c2a7ededed938c7208018c720901938c720a018c720b01938c720c01720e938c720f01721193b172027311959172137312d802d61f9c721399721ba273137e721905d620b2a5731400ededed929a997206721472079c7e9995907219721a72199a721a7315731605721c937213f0721d93721ff0721eedededed93cbc272207317938602720e7213b2db630872207318009386027211721fb2db63087220731900e6c67220040893e4c67220050e8c720401958f7213731aededec929a997206721472079c7e9995907219721a72199a721a731b731c05721c92a39a9a72159c721a7217b27205731d0093721df0721392721e95917219721a731e9c721d99721ba2731f7e721905d801d61fe4c672010704edededed90721f9972197320909972149c7e99721a721f05721c9a721c7207907ef0998c7208027214069d9c7e721c067e721e067e997212732106937213732293721d7323",
        "assets": [
            {
                "tokenId": "48ad28d9bb55e1da36d27c655a84279ff25d889063255d3f774ff926a3704370",
                "amount": 1
            },
            {
                "tokenId": "0779ec04f2fae64e87418a1ad917639d4668f78484f45df962b0dec14a2591d2",
                "amount": 300000
            },
            {
                "tokenId": "98da76cecb772029cfec3d53727d5ff37d5875691825fbba743464af0c89ce45",
                "amount": 1
            },
            {
                "tokenId": "3fdce3da8d364f13bca60998c20660c79c19923f44e141df01349d2e63651e86",
                "amount": 100000000
            },
            {
                "tokenId": "c256908dd9fd477bde350be6a41c0884713a1b1d589357ae731763455ef28c10",
                "amount": 1500000000
            }
        ],
        "creationHeight": 921585,
        "additionalRegisters": {
            "R4": "100490031eaac170c801",
            "R5": "05becf24",
            "R6": "05d00f"
        },
        "transactionId": "ea34ff4653ce1579cb96464014ca072d1574fc04ac58f159786c3a1debebac2b",
        "index": 0
    }"#;
}
