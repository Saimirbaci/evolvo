use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    window, Blob, BlobPropertyBag, Event, FileReader, MediaRecorder, MediaRecorderOptions,
    MediaStream, MediaStreamConstraints,
};

#[derive(Clone)]
pub struct VoiceState {
    pub is_recording: RwSignal<bool>,
    pub base64: RwSignal<Option<String>>,
    pub mime_type: RwSignal<Option<String>>,
    pub transcript: RwSignal<String>,
    pub status_message: RwSignal<Option<String>>,
}

impl VoiceState {
    pub fn new() -> Self {
        Self {
            is_recording: RwSignal::new(false),
            base64: RwSignal::new(None),
            mime_type: RwSignal::new(None),
            transcript: RwSignal::new(String::new()),
            status_message: RwSignal::new(None),
        }
    }

    pub fn clear(&self) {
        self.base64.set(None);
        self.mime_type.set(None);
        self.transcript.set(String::new());
        self.status_message.set(None);
    }
}

impl Default for VoiceState {
    fn default() -> Self {
        Self::new()
    }
}

#[component]
pub fn VoiceRecorder(state: VoiceState) -> impl IntoView {
    // Keeps the active MediaRecorder alive so we can call `.stop()` later.
    let recorder: StoredValue<Option<MediaRecorder>> = StoredValue::new(None);

    let start = {
        let state = state.clone();
        move || {
            let state = state.clone();
            leptos::task::spawn_local(async move {
                match begin_recording(state.clone(), recorder).await {
                    Ok(()) => {
                        state.is_recording.set(true);
                        state.status_message.set(Some("Recording…".into()));
                    }
                    Err(err) => {
                        state
                            .status_message
                            .set(Some(format!("Microphone error: {err}")));
                    }
                }
            });
        }
    };

    let stop = {
        let state = state.clone();
        move || {
            recorder.update_value(|slot| {
                if let Some(rec) = slot.as_ref() {
                    let _ = rec.stop();
                }
                *slot = None;
            });
            state.is_recording.set(false);
            state.status_message.set(Some("Encoding…".into()));
        }
    };

    let toggle = {
        let state = state.clone();
        move |_| {
            if state.is_recording.get_untracked() {
                stop();
            } else {
                state.clear();
                start();
            }
        }
    };

    view! {
        <div class="voice-wrap">
            <button
                class="voice-btn"
                class:recording=move || state.is_recording.get()
                on:click=toggle
            >
                {move || {
                    if state.is_recording.get() {
                        "⏹ Stop recording"
                    } else if state.base64.get().is_some() {
                        "🎙 Record again"
                    } else {
                        "🎙 Record voice note"
                    }
                }}
            </button>

            {move || {
                match state.status_message.get() {
                    Some(msg) => view! { <div class="voice-transcript">{msg}</div> }.into_any(),
                    None => {
                        let transcript = state.transcript.get();
                        if transcript.trim().is_empty() {
                            view! { <span></span> }.into_any()
                        } else {
                            view! { <div class="voice-transcript">{transcript}</div> }.into_any()
                        }
                    }
                }
            }}
        </div>
    }
}

async fn begin_recording(
    state: VoiceState,
    slot: StoredValue<Option<MediaRecorder>>,
) -> Result<(), String> {
    let win = window().ok_or_else(|| "window missing".to_string())?;
    let nav = win.navigator();
    let devices = nav
        .media_devices()
        .map_err(|_| "media devices unavailable".to_string())?;

    let constraints = MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    let stream_promise = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| format!("getUserMedia rejected: {}", fmt_js(&e)))?;
    let stream_js = JsFuture::from(stream_promise)
        .await
        .map_err(|e| format!("microphone denied: {}", fmt_js(&e)))?;
    let stream: MediaStream = stream_js
        .dyn_into()
        .map_err(|_| "unexpected stream type".to_string())?;

    let mime = pick_supported_mime();
    let opts = MediaRecorderOptions::new();
    opts.set_mime_type(&mime);
    let recorder = MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &opts)
        .or_else(|_| MediaRecorder::new_with_media_stream(&stream))
        .map_err(|e| format!("MediaRecorder unsupported: {}", fmt_js(&e)))?;

    let chunks: Rc<RefCell<Vec<Blob>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let chunks = chunks.clone();
        let on_data = Closure::wrap(Box::new(move |ev: Event| {
            if let Ok(be) = ev.dyn_into::<web_sys::BlobEvent>() {
                if let Some(blob) = be.data() {
                    if blob.size() > 0.0 {
                        chunks.borrow_mut().push(blob);
                    }
                }
            }
        }) as Box<dyn FnMut(Event)>);
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
        on_data.forget();
    }

    {
        let stream = stream.clone();
        let chunks = chunks.clone();
        let state = state.clone();
        let mime = mime.clone();
        let on_stop = Closure::wrap(Box::new(move |_: Event| {
            stop_tracks(&stream);
            finish_recording(state.clone(), chunks.clone(), mime.clone());
        }) as Box<dyn FnMut(Event)>);
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
        on_stop.forget();
    }

    {
        let state = state.clone();
        let on_error = Closure::wrap(Box::new(move |_: Event| {
            state
                .status_message
                .set(Some("Recording error".into()));
        }) as Box<dyn FnMut(Event)>);
        recorder.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();
    }

    recorder
        .start()
        .map_err(|e| format!("recorder.start failed: {}", fmt_js(&e)))?;

    slot.set_value(Some(recorder));
    Ok(())
}

fn stop_tracks(stream: &MediaStream) {
    let tracks = stream.get_tracks();
    for i in 0..tracks.length() {
        if let Some(track) = tracks.get(i).dyn_ref::<web_sys::MediaStreamTrack>() {
            track.stop();
        }
    }
}

fn finish_recording(state: VoiceState, chunks: Rc<RefCell<Vec<Blob>>>, mime: String) {
    let chunks_vec = chunks.borrow().clone();
    if chunks_vec.is_empty() {
        state.status_message.set(Some("No audio captured".into()));
        return;
    }
    let array = js_sys::Array::new();
    for blob in chunks_vec.iter() {
        array.push(blob);
    }
    let props = BlobPropertyBag::new();
    props.set_type(&mime);
    let combined = match Blob::new_with_blob_sequence_and_options(&array, &props) {
        Ok(b) => b,
        Err(_) => {
            state
                .status_message
                .set(Some("Failed to combine chunks".into()));
            return;
        }
    };

    let reader = match FileReader::new() {
        Ok(r) => r,
        Err(_) => {
            state
                .status_message
                .set(Some("FileReader unavailable".into()));
            return;
        }
    };
    let reader_clone = reader.clone();
    let state_onload = state.clone();
    let mime_onload = mime.clone();
    let onload = Closure::wrap(Box::new(move |_: Event| {
        if let Ok(value) = reader_clone.result() {
            if let Some(data_url) = value.as_string() {
                if let Some(b64) = data_url.split(',').nth(1) {
                    state_onload.base64.set(Some(b64.to_string()));
                    state_onload
                        .mime_type
                        .set(Some(mime_onload.clone()));
                    state_onload
                        .status_message
                        .set(Some("Voice note captured".into()));
                    return;
                }
            }
        }
        state_onload
            .status_message
            .set(Some("Could not encode voice note".into()));
    }) as Box<dyn FnMut(Event)>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    let _ = reader.read_as_data_url(&combined);
    onload.forget();
}

fn pick_supported_mime() -> String {
    const CANDIDATES: [&str; 4] = [
        "audio/webm;codecs=opus",
        "audio/webm",
        "audio/mp4",
        "audio/ogg",
    ];
    for candidate in CANDIDATES {
        if MediaRecorder::is_type_supported(candidate) {
            return candidate.to_string();
        }
    }
    "audio/webm".to_string()
}

fn fmt_js(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(value, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "unknown error".into())
}
