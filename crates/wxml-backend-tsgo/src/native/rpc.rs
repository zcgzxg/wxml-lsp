//! JSON-RPC identifiers and raw payload seams.
//!
//! No transport is implemented in `P3-I`. The types model the identifier rules
//! that the future router must preserve.

use serde_json::Value;

/// Monotonic client request identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates an identifier. The future router starts at 1 per connection.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the wire integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Server callback identifier with the original JSON scalar preserved.
///
/// The engine currently sends strings such as `"api1"`, but the contract
/// requires echoing the original JSON type, so numbers stay distinct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CallbackId {
    /// Numeric callback identifier.
    Number(u64),
    /// String callback identifier.
    String(String),
}

/// Untouched JSON value used until typed params land in `P3-N`.
#[derive(Debug, Clone)]
pub struct RawPayload {
    value: Value,
}

impl RawPayload {
    /// Wraps a JSON value exactly as it will appear on the wire.
    #[must_use]
    pub const fn new(value: Value) -> Self {
        Self { value }
    }

    /// Returns the wrapped JSON value.
    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.value
    }
}
