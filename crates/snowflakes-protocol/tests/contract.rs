use serde_json::{Value, json};
use snowflakes_protocol::{Configuration, MAX_DOCUMENT_BYTES, Settings};

const BASE: &str = include_str!("../../../protocol/fixtures/configuration.json");
const CASES: &str = include_str!("../../../protocol/fixtures/configuration-cases.json");

fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

#[test]
fn common_contract_cases() {
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    for case in cases {
        let mut document: Value = serde_json::from_str(BASE).unwrap();
        if let Some(changes) = case["set"].as_object() {
            for (path, value) in changes {
                let pointer = pointer(path);
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                document
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert(key.to_owned(), value.clone());
            }
        }
        if let Some(removals) = case["remove"].as_array() {
            for path in removals {
                let pointer = pointer(path.as_str().unwrap());
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                document
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
            }
        }
        let result = Configuration::from_json(&serde_json::to_vec(&document).unwrap());
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
    }
}

#[test]
fn rejects_ambiguous_or_oversized_documents() {
    for document in [
        BASE.replacen(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"schema_version\": 1",
            1,
        ),
        BASE.replacen(
            "\"intensity_percent\": 35",
            "\"intensity_percent\": 35, \"intensity_\\u0070ercent\": 35",
            1,
        ),
        BASE.replacen("\"revision\": 1", "\"revision\": 1.0", 1),
        format!("{BASE} {{}}"),
        " ".repeat(MAX_DOCUMENT_BYTES + 1),
    ] {
        assert!(Configuration::from_json(document.as_bytes()).is_err());
    }
}

#[test]
fn defaults_and_four_controls_round_trip_without_coupling() {
    let mut configuration = Configuration::from_json(BASE.as_bytes()).unwrap();
    assert_eq!(configuration.settings, Settings::default());
    configuration.settings.snow.intensity_percent = 0;
    configuration.settings.snow.drift_size_percent = 100;
    configuration.settings.rain.intensity_percent = 100;
    configuration.settings.rain.puddle_size_percent = 0;
    let copy = Configuration::from_json(&serde_json::to_vec(&configuration).unwrap()).unwrap();
    assert_eq!(configuration, copy);
    assert_eq!(
        serde_json::to_value(copy.settings.rain).unwrap()["intensity_percent"],
        json!(100)
    );

    for global in [true, false] {
        for profile in [true, false] {
            configuration.global_enabled = global;
            configuration.settings.effects_enabled = profile;
            assert_eq!(configuration.effects_enabled(), global && profile);
        }
    }
}
