use serde::Serialize;
use serde_json::Value;

use crate::error::Result;

pub(crate) fn create_tools_json_for_responses_api<T: Serialize>(tools: &[T]) -> Result<Vec<Value>> {
    tools
        .iter()
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()
        .map_err(Into::into)
}
