import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  workers: 1,
  fullyParallel: false,
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width: 1440, height: 1100 },
    permissions: ["camera", "microphone"],
    launchOptions: {
      channel: "chrome",
      args: [
        "--use-fake-device-for-media-stream",
        "--use-fake-ui-for-media-stream",
        "--autoplay-policy=no-user-gesture-required",
      ],
    },
    trace: "retain-on-failure",
  },
  webServer: [
    {
      command: "cargo run -p parentview-signaling",
      url: "http://127.0.0.1:8787/health",
      reuseExistingServer: false,
      timeout: 120_000,
    },
    {
      command: "bun run build:web && bun run --cwd apps/desktop preview --port 1420",
      url: "http://localhost:1420",
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
});
