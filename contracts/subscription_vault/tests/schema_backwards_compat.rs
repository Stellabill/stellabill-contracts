#![cfg(test)]

use subscription_vault::EVENT_SCHEMA_VERSION;

#[derive(Debug, Eq, PartialEq)]
enum SchemaCompatibilityError {
    RemovedField {
        index: usize,
        expected: String,
    },
    ReorderedField {
        index: usize,
        expected: String,
        found: String,
    },
    VersionMismatch {
        expected: u32,
        found: u32,
    },
}

fn fields_from_snapshot(snapshot: &str) -> Vec<String> {
    snapshot
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !line.starts_with("  ") || trimmed.is_empty() {
                return None;
            }
            trimmed
                .split_once(':')
                .map(|(field, _)| field.trim().to_string())
        })
        .collect()
}

fn schema_version_from_snapshot(snapshot: &str) -> Option<u32> {
    snapshot.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("schema_version:")
            .and_then(|value| value.trim().parse::<u32>().ok())
    })
}

fn assert_append_only(
    old_fields: &[String],
    new_fields: &[String],
) -> Result<(), SchemaCompatibilityError> {
    for (index, expected) in old_fields.iter().enumerate() {
        let Some(found) = new_fields.get(index) else {
            return Err(SchemaCompatibilityError::RemovedField {
                index,
                expected: expected.clone(),
            });
        };

        if found != expected {
            return Err(SchemaCompatibilityError::ReorderedField {
                index,
                expected: expected.clone(),
                found: found.clone(),
            });
        }
    }

    Ok(())
}

fn assert_schema_version(snapshot: &str) -> Result<(), SchemaCompatibilityError> {
    let found = schema_version_from_snapshot(snapshot).unwrap_or_default();
    if found != EVENT_SCHEMA_VERSION {
        return Err(SchemaCompatibilityError::VersionMismatch {
            expected: EVENT_SCHEMA_VERSION,
            found,
        });
    }

    Ok(())
}

#[test]
fn subscription_created_v1_fixture_is_strict_prefix_of_v2() {
    let v1 = fields_from_snapshot(include_str!("snapshots/subscription_created_event_v1.txt"));
    let v2_snapshot = include_str!("snapshots/subscription_created_event.txt");
    let v2 = fields_from_snapshot(v2_snapshot);

    assert_append_only(&v1, &v2).expect("v2 must preserve v1 field order");
    assert!(v2.len() > v1.len(), "v2 must append at least one field");
    assert_eq!(v2.last().map(String::as_str), Some("schema_version"));
    assert_schema_version(v2_snapshot).expect("v2 fixture must carry the current version");
}

#[test]
fn nonce_consumed_v1_fixture_is_strict_prefix_of_v2() {
    let v1 = fields_from_snapshot(include_str!("snapshots/nonce_consumed_event_v1.txt"));
    let v2_snapshot = include_str!("snapshots/nonce_consumed_event.txt");
    let v2 = fields_from_snapshot(v2_snapshot);

    assert_append_only(&v1, &v2).expect("v2 must preserve v1 field order");
    assert!(v2.len() > v1.len(), "v2 must append at least one field");
    assert_eq!(v2.last().map(String::as_str), Some("schema_version"));
    assert_schema_version(v2_snapshot).expect("v2 fixture must carry the current version");
}

#[test]
fn removed_field_is_reported_as_schema_break() {
    let old = vec!["subscription_id".to_string(), "subscriber".to_string()];
    let new = vec!["subscription_id".to_string()];

    assert_eq!(
        assert_append_only(&old, &new),
        Err(SchemaCompatibilityError::RemovedField {
            index: 1,
            expected: "subscriber".to_string(),
        })
    );
}

#[test]
fn reordered_field_is_reported_as_schema_break() {
    let old = vec!["subscription_id".to_string(), "subscriber".to_string()];
    let new = vec!["subscriber".to_string(), "subscription_id".to_string()];

    assert_eq!(
        assert_append_only(&old, &new),
        Err(SchemaCompatibilityError::ReorderedField {
            index: 0,
            expected: "subscription_id".to_string(),
            found: "subscriber".to_string(),
        })
    );
}

#[test]
fn trailing_additive_field_is_accepted() {
    let old = vec!["subscription_id".to_string(), "subscriber".to_string()];
    let new = vec![
        "subscription_id".to_string(),
        "subscriber".to_string(),
        "schema_version".to_string(),
    ];

    assert_append_only(&old, &new).expect("trailing fields are additive");
}

#[test]
fn version_mismatch_is_reported_as_schema_break() {
    let stale_snapshot = "data:\n  subscription_id: <u32>\n  schema_version: 1\n";

    assert_eq!(
        assert_schema_version(stale_snapshot),
        Err(SchemaCompatibilityError::VersionMismatch {
            expected: EVENT_SCHEMA_VERSION,
            found: 1,
        })
    );
}
