// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{bail, Result};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;
use tracing::{debug, warn};

static MACOS_26_OR_LATER: OnceLock<bool> = OnceLock::new();

use crate::intelligence::classifier::prompt;
use crate::intelligence::classifier::{ClassifyRequest, ClassifyResponse};

extern "C" {
    fn fm_check_availability(out_reason: *mut *mut c_char) -> i32;
    fn fm_free_string(ptr: *mut c_char);
    fn fm_generate_text(
        instructions: *const c_char,
        prompt: *const c_char,
        out_text: *mut *mut c_char,
        out_error: *mut *mut c_char,
    ) -> i32;
    fn fm_generate_category(
        instructions: *const c_char,
        prompt: *const c_char,
        out_text: *mut *mut c_char,
        out_error: *mut *mut c_char,
    ) -> i32;
}

unsafe fn take_cstring(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    fm_free_string(ptr);
    Some(s)
}

fn is_macos_26_or_later() -> bool {
    *MACOS_26_OR_LATER.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|v| {
                v.trim()
                    .split('.')
                    .next()
                    .and_then(|m| m.parse::<u32>().ok())
            })
            .map(|major| major >= 26)
            .unwrap_or(false)
    })
}

pub struct FoundationBackend;

impl FoundationBackend {
    pub fn is_available() -> bool {
        is_macos_26_or_later()
    }

    pub fn availability_status() -> String {
        if !is_macos_26_or_later() {
            return "macOS 26+ required".to_string();
        }
        unsafe {
            let mut reason: *mut c_char = std::ptr::null_mut();
            fm_check_availability(&mut reason);
            take_cstring(reason).unwrap_or_else(|| "unknown".to_string())
        }
    }

    fn call_generate_text(instructions: &str, user_prompt: &str) -> Result<String> {
        let inst_c = CString::new(instructions)?;
        let prompt_c = CString::new(user_prompt)?;
        let mut out_text: *mut c_char = std::ptr::null_mut();
        let mut out_error: *mut c_char = std::ptr::null_mut();

        let status = unsafe {
            fm_generate_text(
                inst_c.as_ptr(),
                prompt_c.as_ptr(),
                &mut out_text,
                &mut out_error,
            )
        };

        unsafe {
            if status != 0 {
                let err = take_cstring(out_error).unwrap_or_else(|| "unknown error".to_string());
                take_cstring(out_text);
                bail!("Foundation Models error: {}", err);
            }
            let text = take_cstring(out_text).unwrap_or_default();
            take_cstring(out_error);
            Ok(text)
        }
    }

    #[tracing::instrument(
        skip(self, system, user),
        fields(
            backend = "foundation",
            model = "foundation_models",
            prompt_len = user.len(),
            latency_ms = tracing::field::Empty,
        )
    )]
    pub async fn raw_generate(&self, system: &str, user: &str) -> Result<String> {
        if !is_macos_26_or_later() {
            anyhow::bail!("Foundation Models requires macOS 26+");
        }
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(prompt = %user, "llm prompt");
        }
        let start = std::time::Instant::now();
        let system_s = system.to_owned();
        let user_s = user.to_owned();
        let text = tokio::task::spawn_blocking(move || Self::call_generate_text(&system_s, &user_s))
            .await??;
        tracing::Span::current().record("latency_ms", start.elapsed().as_millis() as i64);
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(response = %text, "llm response");
        }
        Ok(text)
    }

    #[tracing::instrument(
        skip(self, req),
        fields(
            backend = "foundation",
            model = "foundation_models",
            app_name = %req.app_name,
            latency_ms = tracing::field::Empty,
            decision = tracing::field::Empty,
        )
    )]
    pub async fn classify(&self, req: &ClassifyRequest) -> Result<ClassifyResponse> {
        if !is_macos_26_or_later() {
            bail!("Foundation Models requires macOS 26+");
        }

        let (system, user) = prompt::build_prompts(req);
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(prompt = %user, "llm prompt");
        }
        let valid_keys = req.valid_keys.clone();
        let start = std::time::Instant::now();

        // fm_generate_text is blocking (DispatchSemaphore) — must run off the async executor
        let result =
            tokio::task::spawn_blocking(move || Self::call_generate_text(&system, &user)).await?;

        tracing::Span::current().record("latency_ms", start.elapsed().as_millis() as i64);

        match result {
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unsupported language")
                    || msg.contains("unsupported Language")
                    || msg.contains("context window")
                {
                    warn!(error = %e, "Foundation Models skipped session — unsupported language or prompt too large");
                    tracing::Span::current().record("decision", "skip");
                    return Ok(ClassifyResponse {
                        task_key: None,
                        method: "foundation_models_skip".to_string(),
                    });
                }
                Err(e)
            }
            Ok(text) => {
                debug!(raw = %text, "Foundation Models raw response");
                if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
                    tracing::trace!(response = %text, "llm response");
                }
                let task_key = prompt::extract_key(&text, &valid_keys);
                tracing::Span::current().record(
                    "decision",
                    task_key.as_deref().unwrap_or("none"),
                );
                Ok(ClassifyResponse {
                    task_key,
                    method: "foundation_models".to_string(),
                })
            }
        }
    }
}
