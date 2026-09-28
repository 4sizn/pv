use serde::Serialize;

/// Host facts, not a promise that an unimplemented native adapter is available.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeCapabilities {
    platform: &'static str,
    native_media: bool,
    remote_input: bool,
    browser_validation_only: bool,
}

#[tauri::command]
fn native_capabilities() -> NativeCapabilities {
    NativeCapabilities {
        platform: std::env::consts::OS,
        native_media: false,
        remote_input: false,
        browser_validation_only: true,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![native_capabilities])
        .run(tauri::generate_context!())
        .expect("desktop host failed to start");
}

#[cfg(test)]
mod tests {
    use super::native_capabilities;

    #[test]
    fn unfinished_adapters_are_never_reported_as_supported() {
        let capabilities = native_capabilities();
        assert!(!capabilities.native_media);
        assert!(!capabilities.remote_input);
        assert!(capabilities.browser_validation_only);
    }
}
