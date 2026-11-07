//! The small JSON Schema vocabulary used by hc's generated result types.
//! Unknown validation keywords fail closed, including after a schema refresh.
use serde_json::Value;
use std::sync::OnceLock;

fn document() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| {
        serde_json::from_str(include_str!("../schema/hc-0.1.18.openrpc.json"))
            .expect("vendored hc schema is valid JSON")
    })
}

pub(crate) fn result(method: &str, value: &Value) -> Result<(), String> {
    let doc = document();
    let schema = doc["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == method)
        .ok_or_else(|| format!("unknown schema method {method}"))?;
    check(doc, &schema["result"]["schema"], value, "result").map_err(|e| format!("{method}: {e}"))
}

pub(crate) fn params(method: &str, value: &Value) -> Result<(), String> {
    let doc = document();
    let method = doc["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == method)
        .ok_or("unknown hc RPC method")?;
    let mut schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
    for param in method["params"].as_array().unwrap() {
        let name = param["name"].as_str().unwrap();
        schema["properties"][name] = param["schema"].clone();
        if param["required"] == true {
            schema["required"].as_array_mut().unwrap().push(name.into());
        }
    }
    check(doc, &schema, value, "params")
}

fn check(doc: &Value, s: &Value, v: &Value, path: &str) -> Result<(), String> {
    if let Some(b) = s.as_bool() {
        return if b {
            Ok(())
        } else {
            Err(format!("{path}: forbidden value"))
        };
    }
    for k in s.as_object().ok_or("schema must be an object")?.keys() {
        if ![
            "$ref",
            "type",
            "properties",
            "required",
            "items",
            "additionalProperties",
            "anyOf",
            "enum",
            "minimum",
            "maximum",
            "minItems",
            "maxItems",
            "description",
            "default",
            "format",
        ]
        .contains(&k.as_str())
        {
            return Err(format!("unsupported schema keyword {k}"));
        }
    }
    if let Some(r) = s["$ref"].as_str() {
        let target = r
            .strip_prefix('#')
            .and_then(|p| doc.pointer(p))
            .ok_or_else(|| format!("unresolved schema reference {r}"))?;
        check(doc, target, v, path)?;
    }
    if let Some(branches) = s["anyOf"].as_array() {
        if !branches.iter().any(|b| check(doc, b, v, path).is_ok()) {
            return Err(format!("{path}: no anyOf branch matches {v}"));
        }
    }
    if let Some(values) = s["enum"].as_array() {
        if !values.contains(v) {
            return Err(format!("{path}: invalid enum value {v}"));
        }
    }
    let matches = |t: &str| match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64(),
        _ => false,
    };
    if let Some(t) = s.get("type") {
        let valid = if let Some(t) = t.as_str() {
            matches(t)
        } else {
            t.as_array()
                .ok_or("invalid schema type")?
                .iter()
                .any(|t| t.as_str().is_some_and(matches))
        };
        if !valid {
            return Err(format!("{path}: expected {t}, got {v}"));
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(required) = s["required"].as_array() {
            for key in required {
                let key = key.as_str().unwrap();
                if !obj.contains_key(key) {
                    return Err(format!("{path}.{key}: required field missing"));
                }
            }
        }
        for (key, value) in obj {
            if let Some(prop) = s["properties"]
                .get(key)
                .or_else(|| s.get("additionalProperties"))
            {
                check(doc, prop, value, &format!("{path}.{key}"))?;
            }
        }
    }
    if let Some(items) = v.as_array() {
        for (key, too_many) in [("minItems", false), ("maxItems", true)] {
            if let Some(bound) = s[key].as_u64() {
                if if too_many {
                    items.len() as u64 > bound
                } else {
                    (items.len() as u64) < bound
                } {
                    return Err(format!("{path}: violates {key}"));
                }
            }
        }
        if let Some(schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                check(doc, schema, item, &format!("{path}[{i}]"))?;
            }
        }
    }
    if let Some(n) = v.as_f64() {
        if s["minimum"].as_f64().is_some_and(|b| n < b)
            || s["maximum"].as_f64().is_some_and(|b| n > b)
        {
            return Err(format!("{path}: number outside schema bounds"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pinned_schema_uses_only_supported_vocabulary() {
        fn walk(s: &Value) {
            if s.is_boolean() {
                return;
            }
            for (key, value) in s.as_object().unwrap() {
                match key.as_str() {
                    "properties" => {
                        for prop in value.as_object().unwrap().values() {
                            walk(prop);
                        }
                    }
                    "items" | "additionalProperties" if value.is_object() => walk(value),
                    "anyOf" => {
                        for branch in value.as_array().unwrap() {
                            walk(branch);
                        }
                    }
                    "$ref"
                    | "type"
                    | "required"
                    | "enum"
                    | "minimum"
                    | "maximum"
                    | "minItems"
                    | "maxItems"
                    | "items"
                    | "additionalProperties"
                    | "description"
                    | "default"
                    | "format" => (),
                    _ => panic!("unsupported schema keyword {key}"),
                }
            }
        }
        let doc = document();
        for schema in doc["components"]["schemas"].as_object().unwrap().values() {
            walk(schema);
        }
        for method in doc["methods"].as_array().unwrap() {
            walk(&method["result"]["schema"]);
            for param in method["params"].as_array().unwrap() {
                walk(&param["schema"]);
            }
        }
    }

    #[test]
    fn validates_nested_values_bounds_enums_and_additional_properties() {
        let doc = document();
        let schema = json!({"type":"object","additionalProperties":{"type":"array","minItems":1,"maxItems":2,"items":{"type":"integer","minimum":0,"maximum":5}}});
        check(doc, &schema, &json!({"x":[0,5]}), "result").unwrap();
        for bad in [
            json!({"x":[]}),
            json!({"x":[0,1,2]}),
            json!({"x":[-1]}),
            json!({"x":[6]}),
            json!({"x":["1"]}),
        ] {
            assert!(check(doc, &schema, &bad, "result").is_err());
        }
        assert!(check(
            doc,
            &json!({"enum":["idle","working"]}),
            &json!("done"),
            "result"
        )
        .is_err());
        assert!(check(doc, &json!({"not":{}}), &Value::Null, "result").is_err());
        assert!(params("terminal.get", &json!({"terminal":"t7","unknown":true})).is_err());
    }
    #[test]
    fn rejects_drift_in_required_fields_types_and_wrapping() {
        let read = json!({"text":"hello","complete":true,"how":"screen"});
        result("terminal.read", &read).unwrap();
        for bad in [
            json!({"text":"hello"}),
            json!({"text":{},"complete":true,"how":"screen"}),
            json!({"result":read}),
        ] {
            assert!(result("terminal.read", &bad).is_err());
        }
        assert!(result(
            "terminal.list",
            &json!({"hidden_finished":0,"terminals":[{"address":"t7"}]})
        )
        .is_err());
        assert!(result("machine.exec", &json!({"schema":1,"trust":null})).is_err());
    }
}
