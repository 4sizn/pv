import {
  createBrowserMediaClient,
  fromBrowserTrack,
  toBrowserTrack,
} from "@parentview/media-sdk/browser";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { BrowserCapture } from "./browser-capture";
import { consumeLocationInvitation } from "./invitation";
import { LabController } from "./lab-controller";
import { LabView } from "./lab-view";
import { LaboratoryApi } from "./laboratory-api";
import { NativeDataProbe } from "./native-data-probe";
import { createTauriNativeMediaClient } from "./native-transport";
import "./styles.css";

interface NativeCapabilities {
  platform: string;
  nativeDataChannels: boolean;
  nativeMedia: boolean;
  remoteInput: boolean;
  browserValidationOnly: boolean;
}

// The composition root is the only place that chooses concrete SDK adapters.
const incomingInvitation = consumeLocationInvitation();
const root = document.getElementById("app");
if (!root) throw new Error("Missing application root.");
const view = new LabView(root, toBrowserTrack);
const lifetime = new AbortController();
let nativeProbe: NativeDataProbe | undefined;
const controller = new LabController(view, new LaboratoryApi(), {
  createClient: (room) =>
    createBrowserMediaClient({
      signalingUrl: `${room.signalingOrigin.replace(/^http/, "ws")}/ws`,
      iceServers: room.iceServers,
    }),
  capture: new BrowserCapture(),
  adaptTrack: fromBrowserTrack,
});
view.setIncomingInvitation(incomingInvitation);
if (isTauri()) {
  void invoke<NativeCapabilities>("native_capabilities")
    .then((capabilities) => {
      if (lifetime.signal.aborted) return;
      if (
        typeof capabilities.nativeDataChannels !== "boolean" ||
        capabilities.nativeMedia ||
        capabilities.remoteInput ||
        !capabilities.browserValidationOnly
      ) {
        throw new Error("호스트의 기능 보고가 이 실험실 빌드와 일치하지 않습니다.");
      }
      view.setRuntime(capabilities.platform);
      if (capabilities.nativeDataChannels) {
        nativeProbe = new NativeDataProbe(view, {
          createClient: createTauriNativeMediaClient,
          createApi: () => new LaboratoryApi(),
        });
        view.enableNativeProbe();
      }
    })
    .catch((error: unknown) => {
      if (!lifetime.signal.aborted) view.showError(error);
    });
}
window.addEventListener(
  "pagehide",
  () => {
    lifetime.abort();
    void Promise.allSettled([nativeProbe?.destroy(), controller.destroy()]);
  },
  { once: true },
);
