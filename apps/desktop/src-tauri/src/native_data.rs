//! Window/document-owned IPC resources only. Session state and negotiation remain in the native SDK.
use pv_media_native::{
    create_client, EventBatch, JoinOptions, NativeDataClient, NativeError, SendResult, Snapshot,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const MAX_CLIENTS: usize = 4;

#[derive(Clone, Default)]
pub struct NativeDataHost(Arc<Mutex<Registry>>);

#[derive(Default)]
struct Registry {
    next_id: u64,
    document: u64,
    closed: bool,
    clients: HashMap<String, Entry>,
}

struct Entry {
    owner: String,
    document: u64,
    client: NativeDataClient,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedClient {
    client_id: String,
    snapshot: Snapshot,
}

impl NativeDataHost {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Registry>, NativeError> {
        self.0
            .lock()
            .map_err(|_| NativeError::new("host_unavailable", "Native client registry unavailable"))
    }

    fn document(&self, owner: &str) -> Result<String, NativeError> {
        if owner != "main" {
            return Err(NativeError::new(
                "forbidden",
                "This window cannot create native clients",
            ));
        }
        let registry = self.lock()?;
        if registry.closed {
            return Err(NativeError::new(
                "host_closed",
                "The native host is closing",
            ));
        }
        Ok(format!("document-{}", registry.document))
    }

    fn open(&self, owner: &str, document_id: &str) -> Result<OpenedClient, NativeError> {
        // The laboratory has one local main window; unrelated webviews never get native clients.
        if owner != "main" {
            return Err(NativeError::new(
                "forbidden",
                "This window cannot create native clients",
            ));
        }
        let mut registry = self.lock()?;
        if registry.closed {
            return Err(NativeError::new(
                "host_closed",
                "The native host is closing",
            ));
        }
        if document_id != format!("document-{}", registry.document) {
            return Err(NativeError::new(
                "stale_document",
                "Native document lease is no longer active",
            ));
        }
        if registry.clients.len() >= MAX_CLIENTS {
            return Err(NativeError::new(
                "client_limit",
                "The native client limit has been reached",
            ));
        }
        let next = registry.next_id.checked_add(1).ok_or_else(|| {
            NativeError::new("client_limit", "Native client identifiers exhausted")
        })?;
        let client = create_client()?;
        let snapshot = client.snapshot();
        let client_id = format!("native-{next}");
        registry.next_id = next;
        let document = registry.document;
        registry.clients.insert(
            client_id.clone(),
            Entry {
                owner: owner.into(),
                document,
                client,
            },
        );
        Ok(OpenedClient {
            client_id,
            snapshot,
        })
    }

    fn client(&self, owner: &str, id: &str) -> Result<NativeDataClient, NativeError> {
        let registry = self.lock()?;
        let entry = registry.clients.get(id).ok_or_else(|| {
            NativeError::new("unknown_client", "Native client handle is not active")
        })?;
        if entry.owner != owner {
            return Err(NativeError::new(
                "forbidden",
                "Native client belongs to another window",
            ));
        }
        if entry.document != registry.document {
            return Err(NativeError::new(
                "stale_document",
                "Native client belongs to a previous document",
            ));
        }
        Ok(entry.client.clone())
    }

    async fn destroy(&self, owner: &str, id: &str) -> Result<Snapshot, NativeError> {
        let client = self.client(owner, id)?;
        // Retain the registry slot until teardown completes, bounding simultaneous native resources.
        let snapshot = client.destroy().await?;
        self.lock()?.clients.remove(id);
        Ok(snapshot)
    }

    /// Reloads abandon JS owners without destroying their native window. Invalidate
    /// leases before teardown; retained slots keep simultaneous native resources bounded.
    pub async fn reset_document(&self, owner: &str) -> Result<(), NativeError> {
        if owner != "main" {
            return Ok(());
        }
        let clients = {
            let mut registry = self.lock()?;
            if registry.closed {
                return Ok(());
            }
            registry.document = registry.document.checked_add(1).ok_or_else(|| {
                NativeError::new("document_limit", "Native document identifiers exhausted")
            })?;
            registry
                .clients
                .iter()
                .map(|(id, entry)| (id.clone(), entry.client.clone()))
                .collect::<Vec<_>>()
        };
        let mut first_error = None;
        for (id, client) in clients {
            match client.destroy().await {
                Ok(_) => {
                    self.lock()?.clients.remove(&id);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub async fn close_window(&self, owner: &str) -> Result<(), NativeError> {
        if owner != "main" {
            return Ok(());
        }
        let clients = {
            let mut registry = self.lock()?;
            registry.closed = true;
            registry
                .clients
                .drain()
                .map(|(_, entry)| entry.client)
                .collect::<Vec<_>>()
        };
        let mut first_error = None;
        for client in clients {
            if let Err(error) = client.destroy().await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[tauri::command]
pub fn native_data_document(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
) -> Result<String, NativeError> {
    host.document(window.label())
}

#[tauri::command]
pub async fn native_data_open(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    document_id: String,
) -> Result<OpenedClient, NativeError> {
    host.open(window.label(), &document_id)
}

#[tauri::command]
pub async fn native_data_read(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    client_id: String,
) -> Result<EventBatch, NativeError> {
    host.client(window.label(), &client_id)?.read_batch().await
}

#[tauri::command]
pub async fn native_data_join(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    client_id: String,
    options: JoinOptions,
) -> Result<Snapshot, NativeError> {
    host.client(window.label(), &client_id)?.join(options).await
}

#[tauri::command]
pub async fn native_data_send(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    client_id: String,
    data: String,
) -> Result<SendResult, NativeError> {
    host.client(window.label(), &client_id)?.send(data).await
}

#[tauri::command]
pub async fn native_data_leave(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    client_id: String,
) -> Result<Snapshot, NativeError> {
    host.client(window.label(), &client_id)?.leave().await
}

#[tauri::command]
pub async fn native_data_destroy(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, NativeDataHost>,
    client_id: String,
) -> Result<Snapshot, NativeError> {
    host.destroy(window.label(), &client_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_media_native::State;
    use std::time::Duration;

    #[tokio::test]
    async fn window_ownership_and_shutdown_are_enforced() {
        let host = NativeDataHost::default();
        assert_eq!(host.document("untrusted").err().unwrap().code, "forbidden");
        let document = host.document("main").unwrap();
        assert_eq!(
            host.open("untrusted", &document).err().unwrap().code,
            "forbidden"
        );
        let opened = host.open("main", &document).unwrap();
        assert_eq!(opened.snapshot.state, State::Idle);
        assert_eq!(
            host.client("other", &opened.client_id).err().unwrap().code,
            "forbidden"
        );
        let client = host.client("main", &opened.client_id).unwrap();
        tokio::time::timeout(Duration::from_secs(5), host.close_window("main"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(client.snapshot().state, State::Destroyed);
        assert!(host.client("main", &opened.client_id).is_err());
        assert_eq!(
            host.open("main", &document).err().unwrap().code,
            "host_closed"
        );
        host.close_window("main").await.unwrap();
    }

    #[tokio::test]
    async fn active_native_handles_are_bounded_and_released_after_destroy() {
        let host = NativeDataHost::default();
        let document = host.document("main").unwrap();
        let mut handles = Vec::new();
        for _ in 0..MAX_CLIENTS {
            handles.push(host.open("main", &document).unwrap().client_id);
        }
        assert_eq!(
            host.open("main", &document).err().unwrap().code,
            "client_limit"
        );
        assert_eq!(
            host.destroy("other", &handles[0]).await.err().unwrap().code,
            "forbidden"
        );
        host.destroy("main", &handles[0]).await.unwrap();
        let replacement = host.open("main", &document).unwrap();
        assert!(!handles.contains(&replacement.client_id));
        host.close_window("main").await.unwrap();
    }

    #[tokio::test]
    async fn document_reload_releases_abandoned_clients_and_rejects_queued_old_opens() {
        let host = NativeDataHost::default();
        let old_document = host.document("main").unwrap();
        let opened = host.open("main", &old_document).unwrap();
        let old_client = host.client("main", &opened.client_id).unwrap();
        tokio::time::timeout(Duration::from_secs(5), host.reset_document("main"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_client.snapshot().state, State::Destroyed);
        assert!(host.client("main", &opened.client_id).is_err());
        assert_eq!(
            host.open("main", &old_document).err().unwrap().code,
            "stale_document"
        );
        let new_document = host.document("main").unwrap();
        assert_ne!(old_document, new_document);
        let current = host.open("main", &new_document).unwrap();
        assert_ne!(opened.client_id, current.client_id);
        assert_eq!(current.snapshot.state, State::Idle);
        host.reset_document("other").await.unwrap();
        assert_eq!(host.document("main").unwrap(), new_document);
        assert!(host.client("main", &current.client_id).is_ok());
        host.close_window("main").await.unwrap();
        host.reset_document("main").await.unwrap();
        assert_eq!(host.document("main").err().unwrap().code, "host_closed");
    }
}
