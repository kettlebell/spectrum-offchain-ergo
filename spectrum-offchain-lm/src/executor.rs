use async_trait::async_trait;
use ergo_lib::chain::transaction::Transaction;
use log::warn;
use type_equalities::trivial_eq;

use spectrum_offchain::backlog::Backlog;
use spectrum_offchain::box_resolver::BoxResolver;
use spectrum_offchain::data::unique_entity::{Predicted, Traced};
use spectrum_offchain::data::{Has, OnChainEntity, OnChainOrder};
use spectrum_offchain::executor::Executor;
use spectrum_offchain::executor::RunOrderFailure;
use spectrum_offchain::network::ErgoNetwork;

use crate::data::bundle::StakingBundle;
use crate::data::order::Order;
use crate::data::pool::Pool;
use crate::data::{BundleId, LmContext};

pub trait RunOrder: OnChainOrder + Sized {
    /// Try to run the given `Order` against the given `Pool`.
    /// Returns transaction, next state of the pool and optionally staking bundle in the case of success.
    /// Returns `RunOrderError<TOrd>` otherwise.
    fn try_run(
        self,
        pool: Pool,
        bundle: Option<StakingBundle>,
        ctx: LmContext,
    ) -> Result<(Transaction, Predicted<Pool>, Option<Predicted<StakingBundle>>), RunOrderFailure<Self>>;
}

pub struct OrderExecutor<TNetwork, TBacklog, TPoolResolver, TBundleResolver> {
    network: TNetwork,
    backlog: TBacklog,
    pool_resolver: TPoolResolver,
    bundle_resolver: TBundleResolver,
    ctx: LmContext,
}

#[async_trait(?Send)]
impl<TNetwork, TBacklog, TPoolResolver, TBundleResolver> Executor
    for OrderExecutor<TNetwork, TBacklog, TPoolResolver, TBundleResolver>
where
    TNetwork: ErgoNetwork,
    TBacklog: Backlog<Order>,
    TPoolResolver: BoxResolver<Pool>,
    TBundleResolver: BoxResolver<StakingBundle>,
{
    async fn execute_next(&mut self) {
        if let Some(ord) = self.backlog.try_pop().await {
            let entity_id = ord.get_entity_ref();
            if let Some(pool) = self.pool_resolver.get(trivial_eq().coerce(entity_id)).await {
                let bundle = if let Some(bundle_id) = ord.get::<Option<BundleId>>() {
                    self.bundle_resolver.get(bundle_id).await
                } else {
                    None
                };
                match ord
                    .clone()
                    .try_run(pool.clone(), bundle.clone(), self.ctx.clone())
                {
                    Ok((tx, next_pool_state, next_bundle_state)) => {
                        if let Err(client_err) = self.network.submit_tx(tx).await {
                            // Note, here `submit_tx(tx)` can fail not only bc pool state is consumed,
                            // but also bc bundle is consumed, what is less possible though. That's why
                            // we just invalidate pool.
                            // todo: In the future more precise error handling may be possible if we
                            // todo: implement a way to find out which input failed exactly.
                            warn!("Execution failed while submitting tx due to {}", client_err);
                            self.pool_resolver
                                .invalidate(pool.get_self_ref(), pool.get_self_state_ref())
                                .await;
                            self.backlog.recharge(ord).await; // Return order to backlog
                        } else {
                            self.pool_resolver
                                .put(Traced {
                                    state: next_pool_state,
                                    prev_state_id: pool.get_self_state_ref(),
                                })
                                .await;
                            if let (Some(next_bundle_state), Some(bundle)) = (next_bundle_state, bundle) {
                                self.bundle_resolver
                                    .put(Traced {
                                        state: next_bundle_state,
                                        prev_state_id: bundle.get_self_state_ref(),
                                    })
                                    .await;
                            }
                        }
                    }
                    Err(RunOrderFailure::NonFatal(err, ord)) => {
                        warn!("Order suspended due to non-fatal error {}", err);
                        self.backlog.suspend(ord).await;
                    }
                    Err(RunOrderFailure::Fatal(err, ord_id)) => {
                        warn!("Order dropped due to fatal error {}", err);
                        self.backlog.remove(ord_id).await;
                    }
                }
            }
        }
    }
}
