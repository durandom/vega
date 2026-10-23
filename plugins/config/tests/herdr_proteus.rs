//! The rendered herdr config matches proteus's config.toml from before vega
//! owned it (the chezmoi version, 2026-10-13) semantically: same keys and
//! values, comments and formatting aside, host rules in any order.
//! Later template additions (the `$upgrade` workspace token, the `$kind`
//! agent token and its colours, and workspace kind slots) are added here too.

use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item};
use vega_config::herdr::{render, TEMPLATE};

const LIVE: &str = include_str!("fixtures/herdr-config-proteus.toml");
const THEME: &str = include_str!("fixtures/theme-proteus.toml");

fn value(v: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match v {
        T::String(s) => Value::from(s.value().as_str()),
        T::Integer(i) => Value::from(*i.value()),
        T::Float(f) => Value::from(*f.value()),
        T::Boolean(b) => Value::from(*b.value()),
        T::Datetime(d) => Value::from(d.value().to_string()),
        T::Array(a) => array(a.iter().map(value).collect()),
        T::InlineTable(t) => {
            Value::Object(t.iter().map(|(k, v)| (k.to_string(), value(v))).collect())
        }
    }
}

/// Rules are matched by distinct `equals` labels: their order is free.
fn array(mut items: Vec<Value>) -> Value {
    if items.iter().all(|i| i.get("equals").is_some()) {
        items.sort_by_key(|i| i["equals"].to_string());
    }
    Value::Array(items)
}

fn item(i: &Item) -> Value {
    match i {
        Item::None => Value::Null,
        Item::Value(v) => value(v),
        Item::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), item(v)))
                .collect::<Map<_, _>>(),
        ),
        Item::ArrayOfTables(a) => Value::Array(
            a.iter()
                .map(|t| Value::Object(t.iter().map(|(k, v)| (k.to_string(), item(v))).collect()))
                .collect(),
        ),
    }
}

fn semantic(text: &str) -> Value {
    item(text.parse::<DocumentMut>().unwrap().as_item())
}

#[test]
fn rendered_matches_proteus_live_config() {
    let sym = vega_herdr::theme::parse(THEME);
    assert!(sym.errors.is_empty(), "{:?}", sym.errors);
    let r = render(TEMPLATE, &sym, Some(LIVE)).unwrap();
    assert_eq!(
        r.kept,
        [("ui.agent_panel_sort".to_string(), "priority".to_string())]
    );
    assert_eq!(semantic(&r.text), semantic(LIVE));
}
