use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

pub fn encode(value: &str) -> String {
    if !value.is_empty()
        && value.is_ascii()
        && value.trim() == value
        && !value.chars().any(char::is_control)
        && !value.starts_with("=?base64?")
    {
        value.to_owned()
    } else {
        format!("=?base64?{}?=", STANDARD.encode(value))
    }
}

pub fn mirror_values(schema: &Value, args: &Value) -> Result<Vec<(String, String)>, String> {
    let mut values = Vec::new();
    for (path, name) in annotations(schema)? {
        let value = path.iter().fold(args, |value, key| &value[key]);
        let text = match value {
            Value::Null => continue,
            Value::String(value) => value.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => {
                let value = value
                    .as_f64()
                    .filter(|n| {
                        n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
                    })
                    .ok_or("Invalid mirrored integer range")?;
                format!("{value:.0}")
            }
            _ => return Err("Invalid mirrored parameter type".into()),
        };
        values.push((format!("Mcp-Param-{name}"), encode(&text)));
    }
    Ok(values)
}

// Validate definitions as well as values; annotations below arrays, refs or
// composition keywords are not statically reachable and cannot be trusted.
pub fn annotations(schema: &Value) -> Result<Vec<(Vec<String>, String)>, String> {
    fn walk(
        node: &Value,
        path: Vec<String>,
        reachable: bool,
        property: bool,
        out: &mut Vec<(Vec<String>, String)>,
    ) -> Result<(), String> {
        if let Some(name) = node.get("x-mcp-header") {
            let name = name.as_str().ok_or("Invalid x-mcp-header")?;
            if !reachable
                || !property
                || !matches!(
                    node["type"].as_str(),
                    Some("string" | "integer" | "boolean")
                )
                || name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || out.iter().any(|(_, seen)| seen.eq_ignore_ascii_case(name))
            {
                return Err("Invalid or duplicate x-mcp-header".into());
            }
            out.push((path.clone(), name.to_owned()));
        }
        if let Some(object) = node.as_object() {
            for (key, value) in object {
                if key == "properties" {
                    if let Some(properties) = value.as_object() {
                        for (name, child) in properties {
                            let mut path = path.clone();
                            path.push(name.clone());
                            walk(child, path, reachable, true, out)?;
                        }
                    }
                } else {
                    walk(value, path.clone(), false, false, out)?;
                }
            }
        } else if let Some(array) = node.as_array() {
            for child in array {
                walk(child, path.clone(), false, false, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(schema, Vec::new(), true, false, &mut out)?;
    Ok(out)
}
