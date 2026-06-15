//! JSON Schema validation for artifact instances.
//!
//! Validates artifact data against the schema declared in its
//! [`ArtifactType`], collecting all violations rather
//! than short-circuiting on the first.

use std::fmt;

use serde_json::Value;

use crate::model::ArtifactType;

/// A single schema violation found during artifact validation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Violation {
    /// The artifact type that was being validated.
    pub artifact_type: String,
    /// Human-readable description of what failed.
    pub description: String,
    /// JSON Pointer into the schema that triggered this violation.
    pub schema_path: String,
    /// JSON Pointer into the instance data where the violation occurred.
    pub instance_path: String,
}

/// Errors that can occur when validating an artifact against its schema.
#[derive(Debug)]
pub enum ValidationError {
    /// The schema itself is malformed or unsupported.
    InvalidSchema {
        artifact_type: String,
        detail: String,
    },
    /// The artifact data violates the schema. Contains all violations found.
    InvalidArtifact {
        artifact_type: String,
        violations: Vec<Violation>,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::InvalidSchema {
                artifact_type,
                detail,
            } => write!(
                f,
                "invalid schema for artifact type '{artifact_type}': {detail}"
            ),
            ValidationError::InvalidArtifact {
                artifact_type,
                violations,
            } => {
                write!(
                    f,
                    "artifact type '{artifact_type}' validation failed ({} violation{}):",
                    violations.len(),
                    if violations.len() == 1 { "" } else { "s" }
                )?;
                for v in violations {
                    write!(f, "\n  - {}: {}", v.schema_path, v.description)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate artifact data against the JSON Schema declared in its `ArtifactType`.
///
/// Returns `Ok(())` if the data conforms to the schema, or an appropriate
/// `ValidationError` if the schema is malformed or the data violates it.
/// All violations are collected so the caller can address them in one pass.
pub fn validate_artifact(
    artifact_data: &Value,
    artifact_type: &ArtifactType,
) -> Result<(), ValidationError> {
    let forge_address_schema = crate::forge_address_schema();
    let validator = jsonschema::options()
        .with_resources(
            [
                (
                    "forge-address.schema.json",
                    jsonschema::Resource::from_contents(forge_address_schema.clone()),
                ),
                (
                    "https://raw.githubusercontent.com/tesserine/groundwork/main/schemas/forge-address.schema.json",
                    jsonschema::Resource::from_contents(forge_address_schema),
                ),
            ]
            .into_iter(),
        )
        .build(&artifact_type.schema)
        .map_err(|e| ValidationError::InvalidSchema {
            artifact_type: artifact_type.name.clone(),
            detail: e.to_string(),
        })?;

    let mut violations: Vec<Violation> = validator
        .iter_errors(artifact_data)
        .map(|e| Violation {
            artifact_type: artifact_type.name.clone(),
            description: e.to_string(),
            schema_path: e.schema_path().to_string(),
            instance_path: e.instance_path().to_string(),
        })
        .collect();

    if violations.is_empty() {
        violations.extend(semantic_violations(artifact_data, artifact_type));
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::InvalidArtifact {
            artifact_type: artifact_type.name.clone(),
            violations,
        })
    }
}

fn semantic_violations(artifact_data: &Value, artifact_type: &ArtifactType) -> Vec<Violation> {
    if artifact_type.name != "work-unit"
        || !schema_references_work_unit_handle(&artifact_type.schema)
    {
        return Vec::new();
    }
    let Some(handle) = artifact_data.get("handle") else {
        return Vec::new();
    };
    match crate::validate_work_unit_handle(handle) {
        Ok(()) => Vec::new(),
        Err(error) => vec![Violation {
            artifact_type: artifact_type.name.clone(),
            description: error.to_string(),
            schema_path: "/$defs/work_unit_handle".to_string(),
            instance_path: "/handle".to_string(),
        }],
    }
}

fn schema_references_work_unit_handle(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            if map
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|reference| reference.ends_with("#/$defs/work_unit_handle"))
            {
                return true;
            }
            map.values().any(schema_references_work_unit_handle)
        }
        Value::Array(values) => values.iter().any(schema_references_work_unit_handle),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_artifact_type(name: &str, schema: Value) -> ArtifactType {
        ArtifactType {
            name: name.into(),
            schema,
        }
    }

    #[test]
    fn valid_artifact_passes() {
        let at = make_artifact_type(
            "report",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "score": { "type": "integer" }
                },
                "required": ["title"]
            }),
        );
        let data = json!({ "title": "Q1 Report", "score": 95 });
        assert!(validate_artifact(&data, &at).is_ok());
    }

    #[test]
    fn missing_required_field_fails() {
        let at = make_artifact_type(
            "report",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" }
                },
                "required": ["title"]
            }),
        );
        let data = json!({ "score": 42 });
        let err = validate_artifact(&data, &at).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidArtifact { .. }));
    }

    #[test]
    fn wrong_type_fails() {
        let at = make_artifact_type(
            "report",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" }
                },
                "required": ["title"]
            }),
        );
        let data = json!({ "title": 123 });
        let err = validate_artifact(&data, &at).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidArtifact { .. }));
    }

    #[test]
    fn nested_object_validation_works() {
        let at = make_artifact_type(
            "design-doc",
            json!({
                "type": "object",
                "properties": {
                    "metadata": {
                        "type": "object",
                        "properties": {
                            "author": { "type": "string" }
                        },
                        "required": ["author"]
                    }
                },
                "required": ["metadata"]
            }),
        );

        // Valid nested object.
        let valid = json!({ "metadata": { "author": "Alice" } });
        assert!(validate_artifact(&valid, &at).is_ok());

        // Missing nested required field.
        let invalid = json!({ "metadata": {} });
        let err = validate_artifact(&invalid, &at).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidArtifact { .. }));
    }

    #[test]
    fn empty_object_fails_with_required_fields() {
        let at = make_artifact_type(
            "constraints",
            json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string" },
                    "priority": { "type": "integer" }
                },
                "required": ["description", "priority"]
            }),
        );
        let data = json!({});
        let err = validate_artifact(&data, &at).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidArtifact { .. }));
    }

    #[test]
    fn complex_schema_validation() {
        let at = make_artifact_type(
            "config",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["name", "tags"],
                "additionalProperties": false
            }),
        );

        // Valid.
        let valid = json!({ "name": "prod", "tags": ["v1", "stable"] });
        assert!(validate_artifact(&valid, &at).is_ok());

        // Empty name violates minLength.
        let bad_name = json!({ "name": "", "tags": [] });
        assert!(validate_artifact(&bad_name, &at).is_err());

        // Extra property violates additionalProperties.
        let extra = json!({ "name": "prod", "tags": [], "extra": true });
        assert!(validate_artifact(&extra, &at).is_err());

        // Non-string in array violates items.
        let bad_tag = json!({ "name": "prod", "tags": [42] });
        assert!(validate_artifact(&bad_tag, &at).is_err());
    }

    #[test]
    fn error_contains_artifact_type_name() {
        let at = make_artifact_type(
            "my-special-type",
            json!({
                "type": "object",
                "required": ["x"]
            }),
        );
        let data = json!({});
        let err = validate_artifact(&data, &at).unwrap_err();
        match err {
            ValidationError::InvalidArtifact {
                artifact_type,
                violations,
            } => {
                assert_eq!(artifact_type, "my-special-type");
                assert!(
                    violations
                        .iter()
                        .all(|v| v.artifact_type == "my-special-type")
                );
            }
            other => panic!("expected InvalidArtifact, got: {other}"),
        }
    }

    #[test]
    fn error_contains_schema_path() {
        let at = make_artifact_type(
            "report",
            json!({
                "type": "object",
                "required": ["title"]
            }),
        );
        let data = json!({});
        let err = validate_artifact(&data, &at).unwrap_err();
        match err {
            ValidationError::InvalidArtifact { violations, .. } => {
                assert!(!violations.is_empty());
                assert!(
                    violations.iter().all(|v| !v.schema_path.is_empty()),
                    "all violations should have a non-empty schema_path"
                );
            }
            other => panic!("expected InvalidArtifact, got: {other}"),
        }
    }

    #[test]
    fn invalid_schema_returns_error() {
        let at = make_artifact_type(
            "broken",
            json!({
                "type": "not-a-real-type"
            }),
        );
        let data = json!({});
        let err = validate_artifact(&data, &at).unwrap_err();
        match err {
            ValidationError::InvalidSchema {
                artifact_type,
                detail,
            } => {
                assert_eq!(artifact_type, "broken");
                assert!(!detail.is_empty());
            }
            other => panic!("expected InvalidSchema, got: {other}"),
        }
    }

    #[test]
    fn work_unit_handle_identity_disagreement_fails_after_schema_validation() {
        let at = make_artifact_type(
            "work-unit",
            json!({
                "type": "object",
                "required": ["title", "description", "acceptance_criteria", "handle"],
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "acceptance_criteria": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "handle": {
                        "$ref": "forge-address.schema.json#/$defs/work_unit_handle"
                    }
                }
            }),
        );
        let data = json!({
            "title": "Bad identity",
            "description": "Stored identity disagrees with its parts.",
            "acceptance_criteria": ["Reject the disagreement"],
            "handle": {
                "tracker": "groundwork",
                "tracker_identity": "sourcehut:todo.weforge.build/~operator/groundwork:4",
                "work_unit_identity": "sourcehut:todo.weforge.build/~operator/groundwork:4#421",
                "number": 420
            }
        });

        let err = validate_artifact(&data, &at).unwrap_err();

        match err {
            ValidationError::InvalidArtifact { violations, .. } => {
                assert_eq!(violations.len(), 1);
                assert_eq!(violations[0].instance_path, "/handle");
                assert!(
                    violations[0]
                        .description
                        .contains("work_unit_identity must be derived"),
                    "{violations:?}"
                );
            }
            other => panic!("expected InvalidArtifact, got: {other}"),
        }
    }

    #[test]
    fn multiple_violations_collected() {
        let at = make_artifact_type(
            "profile",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" },
                    "email": { "type": "string" }
                },
                "required": ["name", "age", "email"]
            }),
        );
        // All three required fields missing.
        let data = json!({});
        let err = validate_artifact(&data, &at).unwrap_err();
        match err {
            ValidationError::InvalidArtifact { violations, .. } => {
                assert!(
                    violations.len() >= 3,
                    "expected at least 3 violations for 3 missing required fields, got {}",
                    violations.len()
                );
                // Root-level required field violations have an empty
                // instance_path (the root object itself). Verify the
                // field is populated consistently.
                assert!(
                    violations.iter().all(|v| v.instance_path.is_empty()),
                    "root-level violations should have empty instance_path"
                );
            }
            other => panic!("expected InvalidArtifact, got: {other}"),
        }
    }

    #[test]
    fn violations_carry_distinct_paths() {
        let at = make_artifact_type(
            "record",
            json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" }
                },
                "required": ["x", "y"]
            }),
        );
        // Both fields present but wrong type.
        let data = json!({ "x": "not-int", "y": "also-not-int" });
        let err = validate_artifact(&data, &at).unwrap_err();
        match err {
            ValidationError::InvalidArtifact { violations, .. } => {
                assert_eq!(violations.len(), 2);
                let schema_paths: Vec<&str> =
                    violations.iter().map(|v| v.schema_path.as_str()).collect();
                assert_ne!(
                    schema_paths[0], schema_paths[1],
                    "violations should have distinct schema paths"
                );
                let instance_paths: Vec<&str> = violations
                    .iter()
                    .map(|v| v.instance_path.as_str())
                    .collect();
                assert_ne!(
                    instance_paths[0], instance_paths[1],
                    "violations should have distinct instance paths"
                );
            }
            other => panic!("expected InvalidArtifact, got: {other}"),
        }
    }
}
