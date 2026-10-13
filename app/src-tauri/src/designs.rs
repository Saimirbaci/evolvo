//! Saved canvas designs: named, reopenable snapshots of the Canvas +
//! Feedback panel (every storyboard screen, the description and the
//! feedback type), plus one autosaved draft so a restart never loses work.
//!
//! The logic lives here as plain functions over `&Store` so tests can drive
//! it without the Tauri macro; `commands.rs` holds the thin wrappers.

use std::collections::HashSet;
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::store::{Store, StoreError, MAX_ATTACHMENT_BYTES};
use crate::types::current_time_unix_ms;
use crate::types::{
    DesignRecord, DesignScreenRecord, DesignSummary, EditableDesign, EditableFeedbackScreen,
    SaveDesignPayload, DESIGN_DRAFT_ID,
};

/// Same ceiling as a storyboard submission.
pub const MAX_DESIGN_SCREENS: usize = 8;
pub const MAX_DESIGN_NAME_CHARS: usize = 80;
pub const MAX_ANNOTATIONS_PER_SCREEN: usize = 5_000;
pub const MAX_IMAGES_PER_SCREEN: usize = 24;
/// Thumbnails are small previews; refuse anything bigger than this.
pub const MAX_THUMBNAIL_BYTES: usize = 1024 * 1024;
/// Total decoded bytes across every image in one save.
pub const MAX_DESIGN_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 20_000;
const MAX_FIELD_CHARS: usize = 500;
pub const DEFAULT_DESIGN_NAME: &str = "Untitled design";

/// Serialises design writes host-wide. Autosave and a manual save can race;
/// asset GC must never run while another save is between writing assets
/// and writing its record.
static DESIGN_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn store_err(e: StoreError) -> String {
    e.to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn decode_image(b64: &str, what: &str) -> Result<Vec<u8>, String> {
    // Accept both raw base64 and `data:image/...;base64,` URLs.
    let raw = b64.split_once(',').map(|(_, d)| d).unwrap_or(b64).trim();
    // Reject before decoding: base64 inflates by 4/3.
    if raw.len() / 4 * 3 > MAX_ATTACHMENT_BYTES + 3 {
        return Err(format!(
            "{what} is larger than {MAX_ATTACHMENT_BYTES} bytes"
        ));
    }
    STANDARD
        .decode(raw)
        .map_err(|e| format!("{what} is not valid base64: {e}"))
}

/// Recognised image types; anything else is refused rather than stored.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn mime_for(filename: &str) -> &'static str {
    match filename.rsplit('.').next() {
        Some("jpg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

fn data_url(filename: &str, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime_for(filename),
        STANDARD.encode(bytes)
    )
}

fn shape_count(rec: &DesignRecord) -> usize {
    rec.screens.iter().map(|s| s.annotations.len()).sum()
}

fn summary(store: &Store, rec: &DesignRecord) -> DesignSummary {
    let thumbnail_data_url = rec.thumbnail.as_deref().and_then(|file| {
        store
            .read_design_asset(&rec.id, file)
            .ok()
            .flatten()
            .map(|bytes| data_url(file, &bytes))
    });
    DesignSummary {
        id: rec.id.clone(),
        name: rec.name.clone(),
        screen_count: rec.screens.len(),
        shape_count: shape_count(rec),
        thumbnail_data_url,
        created_at_unix_ms: rec.created_at_unix_ms,
        updated_at_unix_ms: rec.updated_at_unix_ms,
    }
}

/// Mint a fresh `design-<ms>` id, bumping past any existing record so two
/// saves in the same millisecond never collide.
fn mint_design_id(store: &Store) -> Result<String, String> {
    let mut ms = current_time_unix_ms();
    loop {
        let id = format!("design-{ms}");
        if store.load_design(&id).map_err(store_err)?.is_none() {
            return Ok(id);
        }
        ms += 1;
    }
}

/// Save (create or overwrite) a named design.
pub fn save_design_impl(
    store: &Store,
    payload: SaveDesignPayload,
) -> Result<DesignSummary, String> {
    let id = match payload
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(DESIGN_DRAFT_ID) => return Err("use save_design_draft for the draft".into()),
        Some(id) => {
            if !crate::store::is_valid_design_id(id) {
                return Err(format!("invalid design id: {id:?}"));
            }
            if store.load_design(id).map_err(store_err)?.is_none() {
                return Err(format!(
                    "design {id} no longer exists — save it as a new design"
                ));
            }
            id.to_string()
        }
        None => mint_design_id(store)?,
    };
    write_design(store, &id, payload)
}

/// Overwrite the single autosaved draft.
pub fn save_design_draft_impl(
    store: &Store,
    payload: SaveDesignPayload,
) -> Result<DesignSummary, String> {
    write_design(store, DESIGN_DRAFT_ID, payload)
}

fn write_design(
    store: &Store,
    id: &str,
    payload: SaveDesignPayload,
) -> Result<DesignSummary, String> {
    if payload.screens.is_empty() {
        return Err("a design needs at least one screen".into());
    }
    if payload.screens.len() > MAX_DESIGN_SCREENS {
        return Err(format!(
            "a design can have at most {MAX_DESIGN_SCREENS} screens"
        ));
    }
    let name = {
        let trimmed = payload.name.trim();
        if trimmed.is_empty() {
            DEFAULT_DESIGN_NAME.to_string()
        } else {
            truncate_chars(trimmed, MAX_DESIGN_NAME_CHARS)
        }
    };

    // Validate + decode everything before touching disk, so a bad payload
    // never leaves a half-written design behind.
    let mut total_bytes = 0usize;
    let mut decoded_screens: Vec<(DesignScreenRecord, Vec<Vec<u8>>)> =
        Vec::with_capacity(payload.screens.len());
    for (idx, screen) in payload.screens.into_iter().enumerate() {
        if screen.annotations.len() > MAX_ANNOTATIONS_PER_SCREEN {
            return Err(format!(
                "screen {} has more than {MAX_ANNOTATIONS_PER_SCREEN} shapes",
                idx + 1
            ));
        }
        if screen.pasted_images_base64.len() > MAX_IMAGES_PER_SCREEN {
            return Err(format!(
                "screen {} has more than {MAX_IMAGES_PER_SCREEN} images",
                idx + 1
            ));
        }
        let mut images = Vec::with_capacity(screen.pasted_images_base64.len());
        for (n, b64) in screen.pasted_images_base64.iter().enumerate() {
            let bytes = decode_image(b64, &format!("screen {} image {}", idx + 1, n + 1))?;
            if image_extension(&bytes).is_none() {
                return Err(format!(
                    "screen {} image {} is not a PNG, JPEG, GIF or WebP",
                    idx + 1,
                    n + 1
                ));
            }
            total_bytes += bytes.len();
            images.push(bytes);
        }
        let screen_id = {
            let cleaned: String = screen
                .id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .take(64)
                .collect();
            if cleaned.is_empty() {
                format!("screen-{}", idx + 1)
            } else {
                cleaned
            }
        };
        let record = DesignScreenRecord {
            id: screen_id,
            name: truncate_chars(screen.name.trim(), MAX_FIELD_CHARS),
            description: truncate_chars(&screen.description, MAX_TEXT_CHARS),
            intended_route: truncate_chars(screen.intended_route.trim(), MAX_FIELD_CHARS),
            annotations: screen.annotations,
            pasted_images: Vec::new(),
            window_width: screen.window_width,
            window_height: screen.window_height,
        };
        decoded_screens.push((record, images));
    }
    let thumbnail_bytes = match payload.thumbnail_png_base64.as_deref() {
        Some(b64) if !b64.trim().is_empty() => {
            let bytes = decode_image(b64, "thumbnail")?;
            if bytes.len() > MAX_THUMBNAIL_BYTES {
                return Err(format!(
                    "thumbnail is larger than {MAX_THUMBNAIL_BYTES} bytes"
                ));
            }
            if image_extension(&bytes).is_none() {
                return Err("thumbnail is not a PNG, JPEG, GIF or WebP".into());
            }
            total_bytes += bytes.len();
            Some(bytes)
        }
        _ => None,
    };
    if total_bytes > MAX_DESIGN_TOTAL_BYTES {
        return Err(format!(
            "design images total {total_bytes} bytes, over the {MAX_DESIGN_TOTAL_BYTES}-byte limit"
        ));
    }

    let _guard = DESIGN_WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let existing = store.load_design(id).map_err(store_err)?;
    let now = current_time_unix_ms();
    let mut keep: HashSet<String> = HashSet::new();

    let mut screens = Vec::with_capacity(decoded_screens.len());
    for (mut record, images) in decoded_screens {
        for bytes in images {
            let ext = image_extension(&bytes).unwrap_or("png");
            let file = store
                .write_design_asset(id, &bytes, ext)
                .map_err(store_err)?;
            keep.insert(file.clone());
            record.pasted_images.push(file);
        }
        screens.push(record);
    }
    let thumbnail = match thumbnail_bytes {
        Some(bytes) => {
            let ext = image_extension(&bytes).unwrap_or("png");
            let file = store
                .write_design_asset(id, &bytes, ext)
                .map_err(store_err)?;
            keep.insert(file.clone());
            Some(file)
        }
        None => None,
    };

    let rec = DesignRecord {
        id: id.to_string(),
        name,
        feedback_type: payload.feedback_type,
        feedback_text: truncate_chars(&payload.feedback_text, MAX_TEXT_CHARS),
        screens,
        thumbnail,
        source_design_id: payload.source_design_id.filter(|sid| {
            id == DESIGN_DRAFT_ID && crate::store::is_valid_design_id(sid) && sid != DESIGN_DRAFT_ID
        }),
        created_at_unix_ms: existing.as_ref().map_or(now, |e| e.created_at_unix_ms),
        updated_at_unix_ms: now,
    };
    store.save_design(&rec).map_err(store_err)?;
    // Only after the new record is durable: drop images it no longer uses.
    let _ = store.collect_design_assets(id, &keep);
    Ok(summary(store, &rec))
}

/// Saved designs, most recently updated first. The draft is not listed.
pub fn list_designs_impl(store: &Store) -> Result<Vec<DesignSummary>, String> {
    let mut designs: Vec<DesignSummary> = store
        .list_designs()
        .map_err(store_err)?
        .iter()
        .filter(|rec| rec.id != DESIGN_DRAFT_ID && crate::store::is_valid_design_id(&rec.id))
        .map(|rec| summary(store, rec))
        .collect();
    designs.sort_by(|a, b| {
        b.updated_at_unix_ms
            .cmp(&a.updated_at_unix_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(designs)
}

/// Load a design (or the draft) hydrated for the editor. A missing image
/// file becomes an empty URL; the UI drops just that image shape.
pub fn load_design_impl(store: &Store, id: &str) -> Result<Option<EditableDesign>, String> {
    let id = id.trim();
    if !crate::store::is_valid_design_id(id) {
        return Err(format!("invalid design id: {id:?}"));
    }
    let Some(rec) = store.load_design(id).map_err(store_err)? else {
        return Ok(None);
    };
    let screens = rec
        .screens
        .iter()
        .map(|screen| {
            let pasted_image_data_urls = screen
                .pasted_images
                .iter()
                .map(|file| {
                    Ok(store
                        .read_design_asset(&rec.id, file)
                        .map_err(store_err)?
                        .map(|bytes| data_url(file, &bytes))
                        .unwrap_or_default())
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(EditableFeedbackScreen {
                id: screen.id.clone(),
                name: screen.name.clone(),
                description: screen.description.clone(),
                intended_route: screen.intended_route.clone(),
                annotations: screen.annotations.clone(),
                pasted_image_data_urls,
                window_width: screen.window_width,
                window_height: screen.window_height,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(EditableDesign {
        summary: summary(store, &rec),
        source_design_id: rec.source_design_id.clone(),
        feedback_type: rec.feedback_type,
        feedback_text: rec.feedback_text.clone(),
        screens,
    }))
}

pub fn delete_design_impl(store: &Store, id: &str) -> Result<bool, String> {
    let id = id.trim();
    if !crate::store::is_valid_design_id(id) {
        return Err(format!("invalid design id: {id:?}"));
    }
    let _guard = DESIGN_WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    store.delete_design(id).map_err(store_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackType, SaveDesignScreenPayload};
    use serde_json::json;
    use tempfile::tempdir;

    /// Smallest valid PNG (1x1 transparent).
    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        (tmp, store)
    }

    fn screen(images: Vec<String>) -> SaveDesignScreenPayload {
        SaveDesignScreenPayload {
            id: "screen-1".into(),
            name: "Home".into(),
            description: "landing".into(),
            intended_route: "/".into(),
            annotations: vec![
                json!({"type": "rect", "x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2, "color": "#000000", "width": 3}),
                json!({"type": "image", "x": 0.5, "y": 0.5, "w": 0.1, "h": 0.1, "assetIndex": 0}),
            ],
            pasted_images_base64: images,
            window_width: 1280,
            window_height: 800,
        }
    }

    fn payload(name: &str, images: Vec<String>) -> SaveDesignPayload {
        SaveDesignPayload {
            id: None,
            name: name.into(),
            feedback_type: Some(FeedbackType::NewApp),
            feedback_text: "a todo app".into(),
            screens: vec![screen(images)],
            thumbnail_png_base64: Some(PNG_1X1.into()),
            source_design_id: None,
        }
    }

    #[test]
    fn draft_remembers_its_source_design_but_named_designs_do_not() {
        let (_tmp, store) = store();
        let saved = save_design_impl(&store, payload("A", vec![])).unwrap();
        let mut draft = payload("A", vec![]);
        draft.source_design_id = Some(saved.id.clone());
        save_design_draft_impl(&store, draft).unwrap();
        let loaded = load_design_impl(&store, DESIGN_DRAFT_ID).unwrap().unwrap();
        assert_eq!(loaded.source_design_id, Some(saved.id.clone()));

        let mut bogus = payload("B", vec![]);
        bogus.source_design_id = Some("../x".into());
        save_design_draft_impl(&store, bogus).unwrap();
        assert_eq!(
            load_design_impl(&store, DESIGN_DRAFT_ID)
                .unwrap()
                .unwrap()
                .source_design_id,
            None
        );

        let mut named = payload("C", vec![]);
        named.source_design_id = Some(saved.id.clone());
        let c = save_design_impl(&store, named).unwrap();
        assert_eq!(
            load_design_impl(&store, &c.id)
                .unwrap()
                .unwrap()
                .source_design_id,
            None
        );
    }

    fn asset_files(tmp: &std::path::Path, id: &str) -> Vec<String> {
        let dir = tmp.join("designs").join(format!("{id}.assets"));
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    #[test]
    fn save_then_load_round_trips_screens_images_and_metadata() {
        let (_tmp, store) = store();
        let saved =
            save_design_impl(&store, payload("  Login flow  ", vec![PNG_1X1.into()])).unwrap();
        assert!(saved.id.starts_with("design-"));
        assert_eq!(saved.name, "Login flow");
        assert_eq!((saved.screen_count, saved.shape_count), (1, 2));
        assert!(saved
            .thumbnail_data_url
            .unwrap()
            .starts_with("data:image/png;base64,"));

        let loaded = load_design_impl(&store, &saved.id).unwrap().unwrap();
        assert_eq!(loaded.feedback_type, Some(FeedbackType::NewApp));
        assert_eq!(loaded.feedback_text, "a todo app");
        let s = &loaded.screens[0];
        assert_eq!((s.name.as_str(), s.intended_route.as_str()), ("Home", "/"));
        assert_eq!(s.annotations.len(), 2);
        assert_eq!(s.pasted_image_data_urls.len(), 1);
        assert_eq!(
            s.pasted_image_data_urls[0],
            format!("data:image/png;base64,{PNG_1X1}")
        );
    }

    #[test]
    fn overwrite_keeps_id_and_created_at_and_collects_unused_assets() {
        let (tmp, store) = store();
        let first = save_design_impl(&store, payload("A", vec![PNG_1X1.into()])).unwrap();
        // Same image twice in one save is stored once (content-addressed).
        assert_eq!(asset_files(tmp.path(), &first.id).len(), 1);

        let mut update = payload("B", vec![]);
        update.id = Some(first.id.clone());
        update.thumbnail_png_base64 = None;
        let second = save_design_impl(&store, update).unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.name, "B");
        assert_eq!(second.created_at_unix_ms, first.created_at_unix_ms);
        assert!(second.updated_at_unix_ms >= first.updated_at_unix_ms);
        assert!(
            asset_files(tmp.path(), &first.id).is_empty(),
            "unused image collected"
        );
        assert_eq!(list_designs_impl(&store).unwrap().len(), 1);
    }

    #[test]
    fn list_is_newest_first_and_hides_the_draft() {
        let (_tmp, store) = store();
        let a = save_design_impl(&store, payload("A", vec![])).unwrap();
        let b = save_design_impl(&store, payload("B", vec![])).unwrap();
        save_design_draft_impl(&store, payload("draft", vec![])).unwrap();
        let ids: Vec<String> = list_designs_impl(&store)
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&DESIGN_DRAFT_ID.to_string()));
        assert_ne!(a.id, b.id, "same-millisecond saves get distinct ids");
        assert!(load_design_impl(&store, DESIGN_DRAFT_ID).unwrap().is_some());
    }

    #[test]
    fn draft_is_overwritten_and_deletable() {
        let (_tmp, store) = store();
        save_design_draft_impl(&store, payload("one", vec![PNG_1X1.into()])).unwrap();
        save_design_draft_impl(&store, payload("two", vec![])).unwrap();
        let draft = load_design_impl(&store, DESIGN_DRAFT_ID).unwrap().unwrap();
        assert_eq!(draft.summary.name, "two");
        assert!(delete_design_impl(&store, DESIGN_DRAFT_ID).unwrap());
        assert!(load_design_impl(&store, DESIGN_DRAFT_ID).unwrap().is_none());
        assert!(!delete_design_impl(&store, DESIGN_DRAFT_ID).unwrap());
    }

    #[test]
    fn rejects_hostile_ids_without_touching_disk() {
        let (tmp, store) = store();
        for bad in [
            "../feedback/x",
            "design-",
            "design-12a",
            "Design-1",
            "/etc/passwd",
            "",
        ] {
            assert!(load_design_impl(&store, bad).is_err(), "{bad}");
            assert!(delete_design_impl(&store, bad).is_err(), "{bad}");
        }
        let mut p = payload("x", vec![]);
        p.id = Some("../../escape".into());
        assert!(save_design_impl(&store, p).is_err());
        let mut p = payload("x", vec![]);
        p.id = Some(DESIGN_DRAFT_ID.into());
        assert!(
            save_design_impl(&store, p).is_err(),
            "draft only via the draft command"
        );
        assert!(!tmp.path().join("escape.json").exists());
    }

    #[test]
    fn overwriting_a_deleted_design_is_refused() {
        let (_tmp, store) = store();
        let saved = save_design_impl(&store, payload("A", vec![])).unwrap();
        delete_design_impl(&store, &saved.id).unwrap();
        let mut p = payload("A", vec![]);
        p.id = Some(saved.id);
        assert!(save_design_impl(&store, p)
            .unwrap_err()
            .contains("no longer exists"));
    }

    #[test]
    fn validates_payload_before_writing_anything() {
        let (tmp, store) = store();
        let mut empty = payload("x", vec![]);
        empty.screens.clear();
        assert!(save_design_impl(&store, empty).is_err());

        let mut too_many = payload("x", vec![]);
        too_many.screens = (0..=MAX_DESIGN_SCREENS).map(|_| screen(vec![])).collect();
        assert!(save_design_impl(&store, too_many).is_err());

        assert!(save_design_impl(&store, payload("x", vec!["%%%not-base64".into()])).is_err());
        // Valid base64 but not an image.
        assert!(save_design_impl(&store, payload("x", vec![STANDARD.encode(b"hello")])).is_err());

        let huge = STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES + 10]);
        assert!(save_design_impl(&store, payload("x", vec![huge])).is_err());

        assert!(list_designs_impl(&store).unwrap().is_empty());
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("designs"))
            .unwrap()
            .flatten()
            .collect();
        assert!(
            leftovers.is_empty(),
            "rejected saves must not leave files behind"
        );
    }

    #[test]
    fn blank_name_defaults_and_long_name_is_truncated() {
        let (_tmp, store) = store();
        assert_eq!(
            save_design_impl(&store, payload("   ", vec![]))
                .unwrap()
                .name,
            DEFAULT_DESIGN_NAME
        );
        let long = "x".repeat(MAX_DESIGN_NAME_CHARS + 50);
        assert_eq!(
            save_design_impl(&store, payload(&long, vec![]))
                .unwrap()
                .name
                .chars()
                .count(),
            MAX_DESIGN_NAME_CHARS
        );
    }

    #[test]
    fn missing_asset_file_degrades_to_empty_url() {
        let (tmp, store) = store();
        let saved = save_design_impl(&store, payload("A", vec![PNG_1X1.into()])).unwrap();
        for file in asset_files(tmp.path(), &saved.id) {
            std::fs::remove_file(
                tmp.path()
                    .join("designs")
                    .join(format!("{}.assets", saved.id))
                    .join(file),
            )
            .unwrap();
        }
        let loaded = load_design_impl(&store, &saved.id).unwrap().unwrap();
        assert_eq!(
            loaded.screens[0].pasted_image_data_urls,
            vec![String::new()]
        );
        assert!(loaded.summary.thumbnail_data_url.is_none());
    }

    #[test]
    fn corrupt_design_file_is_quarantined_not_fatal() {
        let (tmp, store) = store();
        save_design_impl(&store, payload("ok", vec![])).unwrap();
        std::fs::write(
            tmp.path().join("designs").join("design-1.json"),
            b"{not json",
        )
        .unwrap();
        assert_eq!(list_designs_impl(&store).unwrap().len(), 1);
        assert!(tmp.path().join("designs").join(".corrupted").exists());
    }
}
