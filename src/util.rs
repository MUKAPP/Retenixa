use crate::{
    contracts::{BudgetDetails, BudgetField, DomainError, ErrorCode, Identifier},
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
pub fn validate_budget(bytes: u32) -> Result<()> {
    if !(1..=4_194_304).contains(&bytes) {
        return Err(DomainError::invalid("byte_budget 范围为 1—4194304"));
    }
    Ok(())
}

pub fn payload_bytes(value: &impl Serialize) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

pub fn budget<T: Serialize>(value: T, bytes: u32, pointer: Option<String>) -> Result<T> {
    validate_budget(bytes)?;
    let n = payload_bytes(&value)?;
    if n <= bytes as usize {
        return Ok(value);
    }
    let value = json(&value)?;
    let mut fields = Vec::new();
    append_budget_fields(&value, "", n, &mut fields)?;
    fields.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.field.cmp(&b.field)));
    fields.truncate(8);
    let mut e = DomainError::new(ErrorCode::BudgetTooSmall, "必要逻辑载荷超出预算");
    e.required_bytes = Some(n as u64);
    e.full_result_pointer = pointer.map(Identifier);
    e.budget_details = Some(Box::new(BudgetDetails {
        min_required_bytes: n as u64,
        largest_fields: fields,
    }));
    Err(e)
}

fn append_budget_fields(
    value: &Value,
    path: &str,
    total: usize,
    out: &mut Vec<BudgetField>,
) -> Result<()> {
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            let field = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if matches!(field.as_str(), "saved_context" | "saved_context.snapshot") {
                append_budget_fields(child, &field, total, out)?;
            } else {
                let bytes = payload_bytes(key)? + 1 + payload_bytes(child)?;
                out.push(BudgetField {
                    field,
                    bytes: bytes as u64,
                    percent: 100.0 * bytes as f64 / total as f64,
                });
            }
        }
    }
    Ok(())
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
