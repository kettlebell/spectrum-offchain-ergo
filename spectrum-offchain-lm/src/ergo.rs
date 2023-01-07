use std::ops::Sub;
use derive_more::{Add, Display, Div, From, Into, Mul, Sub, Sum};
use ergo_lib::chain::transaction::prover_result::ProverResult;
use ergo_lib::ergotree_interpreter::sigma_protocol::prover::{ContextExtension, ProofBytes};
use ergo_lib::ergotree_ir::chain::ergo_box::box_value::BoxValue;
use serde::{Deserialize, Serialize};

/// Max amount of tokens allowed in Ergo.
pub const MAX_VALUE: u64 = 0x7fffffffffffffff;
pub const UNIT_VALUE: u64 = 1;

pub fn empty_prover_result() -> ProverResult {
    ProverResult {
        proof: ProofBytes::Empty,
        extension: ContextExtension::empty(),
    }
}

#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Debug,
    Display,
    Sum,
    Add,
    Sub,
    Mul,
    Div,
    Into,
    From,
    Serialize,
    Deserialize,
)]
pub struct NanoErg(u64);

impl NanoErg {
    pub fn safe_sub(self, n: NanoErg) -> Self {
        Self(self.0.saturating_sub(n.0))
    }
}

pub const MIN_SAFE_BOX_VALUE: NanoErg = NanoErg(250_000);
pub const DEFAULT_MINER_FEE: NanoErg = NanoErg(1_000_000);

impl From<BoxValue> for NanoErg {
    fn from(v: BoxValue) -> Self {
        Self(*v.as_u64())
    }
}

impl From<NanoErg> for BoxValue {
    fn from(nerg: NanoErg) -> Self {
        BoxValue::new(nerg.0).unwrap()
    }
}
