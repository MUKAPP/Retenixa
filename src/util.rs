use crate::{
    contracts::{DomainError, ErrorCode, Identifier},
    error::Result,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn canonical(value: &Value) -> Result<String> {
    serde_jcs::to_string(value)
        .map_err(|e| DomainError::invalid(format!("载荷不能进行 JCS 规范化：{e}")))
}
pub fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    validate_tree(&value, 0)?;
    if serde_json::to_vec(&value)?.len() > 1_048_576 {
        return Err(DomainError::invalid("请求超过 1MiB"));
    }
    Ok(serde_json::from_value(value)?)
}
pub fn validate_tree(v: &Value, depth: usize) -> Result<()> {
    if depth > 48 {
        return Err(DomainError::invalid("JSON 嵌套超过 48 层"));
    }
    match v {
        Value::Array(a) => {
            if a.len() > 1000 {
                return Err(DomainError::invalid("数组超过 1000 项"));
            }
            for x in a {
                validate_tree(x, depth + 1)?;
            }
        }
        Value::Object(o) => {
            for x in o.values() {
                validate_tree(x, depth + 1)?;
            }
        }
        Value::String(s) if s.chars().count() > 16384 => {
            return Err(DomainError::invalid("字符串超过 16384 字符"));
        }
        _ => (),
    }
    Ok(())
}
pub fn json<T: Serialize>(v: &T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}
pub fn budget<T: Serialize>(value: T, bytes: u32, pointer: Option<String>) -> Result<T> {
    if bytes == 0 || bytes > 4_194_304 {
        return Err(DomainError::invalid("byte_budget 范围为 1—4194304"));
    }
    let n = serde_json::to_vec(&rmcp::model::CallToolResult::structured(
        serde_json::to_value(&value)?,
    ))?
    .len();
    if n > bytes as usize {
        let mut e = DomainError::new(
            ErrorCode::BudgetTooSmall,
            "完整必要响应超出预算；没有丢弃未知操作或变化摘要",
        );
        e.required_bytes = Some(n as u64);
        e.full_result_pointer = pointer.map(Identifier);
        return Err(e);
    }
    Ok(value)
}
pub fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}
pub fn cursor(value: &Value) -> Result<String> {
    let raw = serde_json::to_vec(value)?;
    Ok(raw.iter().map(|x| format!("{x:02x}")).collect())
}
pub fn decode_cursor(raw: &str) -> Result<Value> {
    if !raw.is_ascii() || raw.len() > 4096 || !raw.len().is_multiple_of(2) {
        return Err(DomainError::invalid("cursor 无效"));
    }
    let bytes = (0..raw.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&raw[i..i + 2], 16).map_err(|_| DomainError::invalid("cursor 无效"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::from_slice(&bytes)?)
}
