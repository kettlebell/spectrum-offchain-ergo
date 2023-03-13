use std::hash::Hash;

use type_equalities::IsEqual;

pub mod order;
pub mod unique_entity;

pub trait Has<T> {
    fn get<U: IsEqual<T>>(&self) -> T;
}

pub trait OnChainOrderId {
    type TOrderId: Eq + Hash;

    fn get_self_ref(&self) -> Self::TOrderId;
}

pub trait OnChainEntityId {
    type TEntityId: Eq + Hash;

    fn get_entity_ref(&self) -> Self::TEntityId;
}

pub trait OnChainEntity {
    type TEntityId: Eq + Hash;
    type TStateId: Eq + Hash;

    fn get_self_ref(&self) -> Self::TEntityId;

    fn get_self_state_ref(&self) -> Self::TStateId;
}
