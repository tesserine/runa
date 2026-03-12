use std::fmt;

use serde_json::Value;

use crate::model::ArtifactType;

/// Errors that can occur when validating an artifact against its schema.
#[derive(Debug)]
pub enum ValidationError {
    /// The schema itself is malformed or unsupported.
    InvalidSchema {
        artifact_type: String,
        detail: String,
    },
    /// The artifact data violates the schema.
    InvalidArtifact {
        artifact_type: String,
        description: String,
        schema_path: String,
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
                description,
                schema_path,
            } => write!(
                f,
                "artifact type '{artifact_type}' validation failed at {schema_path}: {description}"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate artifact data against the JSON Schema declared in its `ArtifactType`.
///
/// Returns `Ok(())` if the data conforms to the schema, or an appropriate
/// `ValidationError` if the schema is malformed or the data violates it.
pub fn validate_artifact(
    artifact_data: &Value,
    artifact_type: &ArtifactType,
) -> Result<(), ValidationError> {
    let validator =
        jsonschema::validator_for(&artifact_type.schema).map_err(|e| {
            ValidationError::InvalidSchema {
                artifact_type: artifact_type.name.clone(),
                detail: e.to_string(),
            }
        })?;

    validator.validate(artifact_data).map_err(|e| {
        ValidationError::InvalidArtifact {
            artifact_type: artifact_type.name.clone(),
            description: e.to_string(),
            schema_path: e.schema_path.to_string(),
        }
    })
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
                artifact_type, ..
            } => assert_eq!(artifact_type, "my-special-type"),
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
            ValidationError::InvalidArtifact { schema_path, .. } => {
                assert!(!schema_path.is_empty(), "schema_path should be non-empty");
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
}
