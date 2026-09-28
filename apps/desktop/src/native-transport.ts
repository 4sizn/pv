import {
  type NativeDataBatch,
  NativeDataClient,
  type NativeDataJoinOptions,
  type NativeDataSendResult,
  type NativeDataSnapshot,
  type NativeDataTransportPort,
} from "@parentview/media-sdk/native";
import { invoke } from "@tauri-apps/api/core";

export type NativeInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

interface OpenedClient {
  readonly clientId: string;
  readonly snapshot: NativeDataSnapshot;
}

/** One document-owned IPC handle. Rust owns all session/peer state; the SDK owns the read pump. */
export class TauriNativeDataTransport implements NativeDataTransportPort {
  readonly #invoke: NativeInvoke;
  #clientId: string | undefined;
  #opening: Promise<NativeDataSnapshot> | undefined;
  #closing: Promise<NativeDataSnapshot> | undefined;
  #disposed = false;

  constructor(options: { invoke?: NativeInvoke } = {}) {
    this.#invoke = options.invoke ?? invoke;
  }

  async open(): Promise<NativeDataSnapshot> {
    if (this.#disposed) throw new Error("Native transport is closed");
    this.#opening ??= this.#open();
    return this.#opening;
  }

  async #open(): Promise<NativeDataSnapshot> {
    // A queued open from an old document must not allocate a client after navigation.
    // Fetching the lease allocates no native resources.
    const documentId = await this.#invoke<unknown>("native_data_document");
    if (this.#disposed) throw new Error("Native transport closed while opening");
    if (typeof documentId !== "string" || documentId.length === 0) {
      throw new Error("Native host returned an invalid document lease");
    }
    const result = await this.#invoke<OpenedClient>("native_data_open", { documentId });
    if (!result || typeof result.clientId !== "string" || result.clientId.length === 0) {
      throw new Error("Native host returned an invalid client handle");
    }
    // Retain the handle before snapshot validation so failed creation can still dispose it.
    this.#clientId = result.clientId;
    if (this.#disposed) throw new Error("Native transport closed while opening");
    return result.snapshot;
  }

  async readBatch(): Promise<NativeDataBatch> {
    return this.#invoke("native_data_read", { clientId: this.#handle() });
  }

  async join(options: NativeDataJoinOptions): Promise<NativeDataSnapshot> {
    return this.#invoke("native_data_join", { clientId: this.#handle(), options });
  }

  async send(data: string): Promise<NativeDataSendResult> {
    return this.#invoke("native_data_send", { clientId: this.#handle(), data });
  }

  async leave(): Promise<NativeDataSnapshot> {
    return this.#invoke("native_data_leave", { clientId: this.#handle() });
  }

  destroy(): Promise<NativeDataSnapshot> {
    if (this.#closing) return this.#closing;
    this.#disposed = true;
    this.#closing = this.#dispose();
    return this.#closing;
  }

  async #dispose(): Promise<NativeDataSnapshot> {
    try {
      await this.#opening;
    } catch {
      // Opening can fail after a valid handle was received; that handle still needs teardown.
    }
    const clientId = this.#clientId;
    if (!clientId) throw new Error("Native transport has no open client to destroy");
    try {
      return await this.#invoke("native_data_destroy", { clientId });
    } finally {
      this.#clientId = undefined;
    }
  }

  #handle(): string {
    if (this.#disposed || !this.#clientId) throw new Error("Native transport is not open");
    return this.#clientId;
  }
}

/** App composition: the reusable SDK never imports Tauri. */
export function createTauriNativeDataClient(): Promise<NativeDataClient> {
  return NativeDataClient.create({ transport: new TauriNativeDataTransport() });
}
