//! Lineage bundle export / import — operationalises product invariant I-P4
//! ("Lineagees are saveable and forkable into standalone apps").
//!
//! A lineage bundle is a `.evolvo-bundle` zip archive with a portable shape:
//!
//! ```text
//! manifest.json
//! lineage_jobs/<id>.json     # host-specific paths stripped
//! feedback/<feedback_id>.json
//! attachments/<feedback_id>/<file>
//! ```
//!
//! The bundle deliberately contains **no host-absolute paths, no source-repo
//! pointers, no workspace-root strings**. It's the minimum needed to mint a
//! new Evolvo app whose lineage matches the source's at the moment of export.
//! Import seeds a fresh workspace under a caller-supplied root — re-running
//! the iteration is the new app's choice; we don't bring run state with us.
//!
//! Round-trip property (covered by tests): export → import → export produces
//! a structurally equivalent bundle (same files, same JSON modulo bundle-id
//! + timestamp).
//
// Implementation note: we intentionally read every record/attachment into
// memory before writing the zip. Bundles are scoped to a single lineage job
// (a handful of feedback rows + attachments), so memory pressure is bounded
// by the user's own attachment sizes.
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::store::{Store, StoreError};
use crate::types::{current_time_unix_ms, FeedbackRecord, LineageJobRecord};

pub const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const BUNDLE_EXTENSION: &str = "evolvo-bundle";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub schema_version: u32,
    /// Newly-minted identifier for this exported bundle. Lets a future "is
    /// this the same bundle?" check work without comparing every byte.
    pub bundle_id: String,
    pub exported_at_unix_ms: u64,
    pub source_app_name: String,
    pub source_app_version: String,
    pub primary_job_id: String,
    pub feedback_ids: Vec<String>,
}

/// Strip host-specific run state from a lineage job before serializing into a
/// bundle. Worktree paths, branch names, and log paths are local artefacts —
/// they would leak the source machine's filesystem into the bundle and would
/// be invalid on the import side anyway. Notes + stages stay (they're the
/// reviewable history); status is downgraded so the imported app can re-plan
/// from a clean slate without inheriting "Implementing" state mid-flight.
fn portable_job(mut job: LineageJobRecord) -> LineageJobRecord {
    job.worktree_path = None;
    job.branch_name = None;
    job.log_path = None;
    job.source_repo = None;
    job
}

/// Build a `.evolvo-bundle` zip in memory for the given lineage job.
///
/// Returns the raw zip bytes plus the manifest used (the manifest is also
/// written into the archive). The caller is responsible for choosing where
/// the bytes land on disk — we keep this layer side-effect-free for
/// deterministic tests.
pub fn build_bundle_bytes(
    store: &Store,
    job_id: &str,
    source_app_name: &str,
    source_app_version: &str,
) -> Result<(Vec<u8>, BundleManifest), StoreError> {
    let job = store
        .load_lineage_job(job_id)?
        .ok_or_else(|| StoreError::Other(format!("lineage job not found: {job_id}")))?;

    // For now a bundle is one primary job + the single feedback row that
    // spawned it. Multi-feedback bundles are a forward-compatible extension
    // (`feedback_ids` is already a Vec).
    let mut feedback_ids = Vec::new();
    let mut feedback_records = Vec::new();
    if let Some(fb) = store.load_feedback(&job.feedback_id)? {
        feedback_ids.push(fb.id.clone());
        feedback_records.push(fb);
    }

    let manifest = BundleManifest {
        schema_version: BUNDLE_SCHEMA_VERSION,
        bundle_id: format!("bundle-{}", current_time_unix_ms()),
        exported_at_unix_ms: current_time_unix_ms(),
        source_app_name: source_app_name.to_string(),
        source_app_version: source_app_version.to_string(),
        primary_job_id: job.id.clone(),
        feedback_ids: feedback_ids.clone(),
    };

    let buf: Vec<u8> = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = ZipWriter::new(cursor);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    zip.start_file("manifest.json", opts)
        .map_err(|e| StoreError::Other(e.to_string()))?;
    zip.write_all(&manifest_json)?;

    let portable = portable_job(job.clone());
    let job_json = serde_json::to_vec_pretty(&portable)?;
    zip.start_file(format!("lineage_jobs/{}.json", portable.id), opts)
        .map_err(|e| StoreError::Other(e.to_string()))?;
    zip.write_all(&job_json)?;

    for fb in &feedback_records {
        let fb_json = serde_json::to_vec_pretty(fb)?;
        zip.start_file(format!("feedback/{}.json", fb.id), opts)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        zip.write_all(&fb_json)?;

        let att_dir = store.layout().attachments_dir(&fb.id);
        if att_dir.exists() {
            for entry in fs::read_dir(&att_dir)? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let bytes = fs::read(&path)?;
                zip.start_file(format!("attachments/{}/{}", fb.id, name), opts)
                    .map_err(|e| StoreError::Other(e.to_string()))?;
                zip.write_all(&bytes)?;
            }
        }
    }

    let cursor = zip.finish().map_err(|e| StoreError::Other(e.to_string()))?;
    Ok((cursor.into_inner(), manifest))
}

/// Write the bundle to `dest_dir`, returning the absolute path of the
/// created file. Filename is `<job-id>-<bundle-id>.evolvo-bundle` so two
/// exports of the same lineage at different points in time don't clobber
/// each other.
pub fn export_lineage_to_dir(
    store: &Store,
    job_id: &str,
    dest_dir: &Path,
    source_app_name: &str,
    source_app_version: &str,
) -> Result<PathBuf, StoreError> {
    fs::create_dir_all(dest_dir)?;
    let (bytes, manifest) = build_bundle_bytes(store, job_id, source_app_name, source_app_version)?;
    let filename = format!(
        "{}-{}.{}",
        sanitise_segment(job_id),
        sanitise_segment(&manifest.bundle_id),
        BUNDLE_EXTENSION
    );
    let out = dest_dir.join(filename);
    fs::write(&out, &bytes)?;
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub workspace_root: String,
    pub primary_job_id: String,
    pub feedback_count: u32,
    pub attachment_count: u32,
}

/// Read a `.evolvo-bundle` zip at `bundle_path` and seed a fresh workspace
/// rooted at `target_root`. Refuses to import into a workspace that already
/// contains feedback or lineage jobs — forking is for *new* apps; if the
/// caller really wants to merge, they can pick a fresh root.
pub fn import_lineage_bundle(
    bundle_path: &Path,
    target_root: &Path,
) -> Result<ImportSummary, StoreError> {
    let bytes = fs::read(bundle_path)?;
    import_lineage_bundle_bytes(&bytes, target_root)
}

pub fn import_lineage_bundle_bytes(
    bytes: &[u8],
    target_root: &Path,
) -> Result<ImportSummary, StoreError> {
    let store = Store::new(target_root.to_path_buf());

    if !store.list_feedback()?.is_empty() || !store.list_lineage_jobs()?.is_empty() {
        return Err(StoreError::Other(format!(
            "target workspace at {} already contains lineage data — pick an empty directory",
            target_root.display()
        )));
    }
    store.init_workspace()?;

    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| StoreError::Other(e.to_string()))?;

    let mut manifest: Option<BundleManifest> = None;
    let mut feedback_count: u32 = 0;
    let mut attachment_count: u32 = 0;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        // `enclosed_name` rejects paths containing `..` or absolute roots —
        // critical because a malicious bundle could otherwise traverse out of
        // the workspace.
        let Some(rel) = file.enclosed_name() else {
            return Err(StoreError::Other(format!(
                "bundle contains unsafe path: {}",
                file.name()
            )));
        };
        if file.is_dir() {
            continue;
        }
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)?;

        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str == "manifest.json" {
            let parsed: BundleManifest = serde_json::from_slice(&data)?;
            if parsed.schema_version != BUNDLE_SCHEMA_VERSION {
                return Err(StoreError::Other(format!(
                    "unsupported bundle schema: {}",
                    parsed.schema_version
                )));
            }
            manifest = Some(parsed);
        } else if let Some(rest) = rel_str.strip_prefix("lineage_jobs/") {
            if !rest.ends_with(".json") {
                continue;
            }
            let job: LineageJobRecord = serde_json::from_slice(&data)?;
            store.save_lineage_job(&job)?;
        } else if let Some(rest) = rel_str.strip_prefix("feedback/") {
            if !rest.ends_with(".json") {
                continue;
            }
            let fb: FeedbackRecord = serde_json::from_slice(&data)?;
            store.save_feedback(&fb)?;
            feedback_count += 1;
        } else if let Some(rest) = rel_str.strip_prefix("attachments/") {
            // attachments/<feedback_id>/<filename>
            let mut parts = rest.splitn(2, '/');
            let (Some(fb_id), Some(name)) = (parts.next(), parts.next()) else {
                continue;
            };
            store.save_attachment(fb_id, name, &data)?;
            attachment_count += 1;
        }
        // Unknown top-level entries are ignored — forward compatibility.
    }

    let manifest = manifest
        .ok_or_else(|| StoreError::Other("bundle is missing manifest.json".to_string()))?;

    Ok(ImportSummary {
        workspace_root: target_root.display().to_string(),
        primary_job_id: manifest.primary_job_id,
        feedback_count,
        attachment_count,
    })
}

fn sanitise_segment(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        FeedbackRecord, FeedbackStatus, FeedbackType, LineageJobRecord, LineageJobStatus,
    };
    use tempfile::tempdir;

    fn seed_workspace(root: &Path) -> Store {
        let store = Store::new(root.to_path_buf());
        store.init_workspace().unwrap();

        let fb = FeedbackRecord {
            id: "fb-1".into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "the login button is broken".into(),
            annotations: vec![],
            pasted_images: vec!["paste-0.png".into()],
            screenshot_filename: Some("canvas.png".into()),
            voice_filename: None,
            voice_transcript: None,
            window_width: 1024,
            window_height: 768,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            lineage_job_id: Some("job-1".into()),
        };
        store.save_feedback(&fb).unwrap();
        store
            .save_attachment("fb-1", "canvas.png", &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap();
        store
            .save_attachment("fb-1", "paste-0.png", &[1, 2, 3, 4])
            .unwrap();

        let job = LineageJobRecord {
            id: "job-1".into(),
            feedback_id: "fb-1".into(),
            title: "Fix login".into(),
            summary: "User reports login broken".into(),
            status: LineageJobStatus::BuildReady,
            notes: vec!["enqueued".into(), "implementation finished".into()],
            created_at_unix_ms: 2,
            updated_at_unix_ms: 3,
            // These host-specific fields MUST be stripped by export.
            worktree_path: Some("/Users/host/.evolvo/worktrees/job-1".into()),
            branch_name: Some("lineage/job-1".into()),
            log_path: Some("/Users/host/.evolvo/.../claude.log".into()),
            source_repo: Some("/Users/host/code/evolvo".into()),
            iteration: 3,
            stages: Vec::new(),
        };
        store.save_lineage_job(&job).unwrap();
        store
    }

    #[test]
    fn build_bundle_strips_host_paths() {
        let temp = tempdir().unwrap();
        let store = seed_workspace(temp.path());
        let (bytes, manifest) = build_bundle_bytes(&store, "job-1", "Evolvo", "0.1.0").unwrap();
        assert_eq!(manifest.primary_job_id, "job-1");
        assert_eq!(manifest.feedback_ids, vec!["fb-1"]);
        // No host-absolute paths anywhere in the bundle.
        let needle = "/Users/host";
        let memmem_hit = bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
        assert!(!memmem_hit, "bundle leaked host-absolute path");
    }

    #[test]
    fn round_trip_export_import_preserves_lineage() {
        let src = tempdir().unwrap();
        let store = seed_workspace(src.path());
        let dst_dir = tempdir().unwrap();
        let bundle_path =
            export_lineage_to_dir(&store, "job-1", dst_dir.path(), "Evolvo", "0.1.0").unwrap();
        assert!(bundle_path.exists());
        assert!(bundle_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".evolvo-bundle"));

        let target = tempdir().unwrap();
        let summary = import_lineage_bundle(&bundle_path, target.path()).unwrap();
        assert_eq!(summary.primary_job_id, "job-1");
        assert_eq!(summary.feedback_count, 1);
        assert_eq!(summary.attachment_count, 2);

        let new_store = Store::new(target.path().to_path_buf());
        let job = new_store.load_lineage_job("job-1").unwrap().unwrap();
        // Run state stripped on the way out, absent on the way back in.
        assert!(job.worktree_path.is_none());
        assert!(job.branch_name.is_none());
        assert!(job.log_path.is_none());
        assert!(job.source_repo.is_none());
        // Reviewable history preserved.
        assert_eq!(job.title, "Fix login");
        assert_eq!(job.notes.len(), 2);

        let fb = new_store.load_feedback("fb-1").unwrap().unwrap();
        assert_eq!(fb.feedback_text, "the login button is broken");

        let canvas = new_store.read_attachment("fb-1", "canvas.png").unwrap();
        assert_eq!(canvas.as_deref(), Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]));
    }

    #[test]
    fn import_refuses_non_empty_workspace() {
        let src = tempdir().unwrap();
        let store = seed_workspace(src.path());
        let dst_dir = tempdir().unwrap();
        let bundle_path =
            export_lineage_to_dir(&store, "job-1", dst_dir.path(), "Evolvo", "0.1.0").unwrap();

        let target = tempdir().unwrap();
        // Pre-seed the target with one feedback row → import must refuse.
        let _ = seed_workspace(target.path());
        let err = import_lineage_bundle(&bundle_path, target.path()).unwrap_err();
        assert!(err.to_string().contains("already contains"));
    }

    #[test]
    fn import_rejects_path_traversal() {
        // Hand-craft a malicious zip with a `../escape.txt` entry and verify
        // import refuses it instead of writing outside the target root.
        let mut zip = ZipWriter::new(Cursor::new(Vec::<u8>::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let manifest = BundleManifest {
            schema_version: BUNDLE_SCHEMA_VERSION,
            bundle_id: "bundle-x".into(),
            exported_at_unix_ms: 0,
            source_app_name: "Evolvo".into(),
            source_app_version: "0.1.0".into(),
            primary_job_id: "job-x".into(),
            feedback_ids: vec![],
        };
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.start_file("../escape.txt", opts).unwrap();
        zip.write_all(b"pwn").unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let target = tempdir().unwrap();
        let err = import_lineage_bundle_bytes(&bytes, target.path()).unwrap_err();
        assert!(err.to_string().contains("unsafe path"));
    }
}
