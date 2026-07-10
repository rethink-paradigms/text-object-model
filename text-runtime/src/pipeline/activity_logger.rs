// ── Activity Logger ─────────────────────────────────────────────────────────
//
// Logs ingestion activities to the activities table (append-only).

use crate::error::TextRuntimeError;
use crate::store::db::DbStore;
use crate::store::types::ActivityRow;
use crate::uuid7::uuid7;

/// Log an ingestion activity.
///
/// Creates an ActivityRow with the given parameters and inserts it into
/// the activities table. Returns the activity UUID.
pub fn log_ingest_activity(
    db: &mut DbStore,
    activity_type: &str,
    input_ids: &[String],
    output_ids: &[String],
    agent: &str,
    config_json: &str,
) -> Result<String, TextRuntimeError> {
    let activity_uuid = uuid7();
    let now = chrono::Utc::now().to_rfc3339();

    let activity = ActivityRow {
        id: 0,
        uuid: activity_uuid.to_string(),
        activity_type: activity_type.to_string(),
        input_ids: if input_ids.is_empty() {
            None
        } else {
            Some(serde_json::to_string(input_ids).map_err(|e| {
                TextRuntimeError::InternalError(format!("failed to serialize input_ids: {}", e))
            })?)
        },
        output_ids: if output_ids.is_empty() {
            None
        } else {
            Some(serde_json::to_string(output_ids).map_err(|e| {
                TextRuntimeError::InternalError(format!("failed to serialize output_ids: {}", e))
            })?)
        },
        agent: if agent.is_empty() {
            None
        } else {
            Some(agent.to_string())
        },
        config: if config_json.is_empty() {
            None
        } else {
            Some(config_json.to_string())
        },
        started_at: now.clone(),
        ended_at: Some(now),
    };

    db.insert_activity(&activity)?;

    Ok(activity_uuid.to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::TempDir;

    fn setup_store() -> (Store, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let runtime_dir = tmp.path().join(".textruntime");
        let store = Store::open(&runtime_dir).expect("open store");
        (store, tmp)
    }

    #[test]
    fn test_log_ingest_activity() {
        let (mut store, _tmp) = setup_store();

        let activity_uuid = log_ingest_activity(
            &mut store.db,
            "ingest",
            &["input-doc-uuid".to_string()],
            &["output-doc-uuid".to_string()],
            "test-agent",
            r#"{"format":"markdown"}"#,
        )
        .expect("log activity");

        assert!(!activity_uuid.is_empty());

        // Verify the activity was persisted
        let activities = store
            .db
            .get_activities(None, Some("ingest"), Some(10))
            .expect("get activities");

        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].uuid, activity_uuid);
        assert_eq!(activities[0].activity_type, "ingest");
        assert!(activities[0].agent.as_deref() == Some("test-agent"));
        assert!(activities[0].input_ids.is_some());
        assert!(activities[0].output_ids.is_some());
        assert!(activities[0].config.is_some());
        assert!(!activities[0].started_at.is_empty());
    }

    #[test]
    fn test_log_reingest_activity() {
        let (mut store, _tmp) = setup_store();

        let activity_uuid = log_ingest_activity(
            &mut store.db,
            "reingest",
            &[],
            &["doc-uuid".to_string()],
            "",
            "",
        )
        .expect("log reingest");

        let activities = store
            .db
            .get_activities(None, Some("reingest"), Some(10))
            .expect("get activities");

        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].uuid, activity_uuid);
        assert!(activities[0].agent.is_none());
        assert!(activities[0].input_ids.is_none());
        assert!(activities[0].config.is_none());
    }
}
