import { mkdir } from "node:fs/promises";
import { expect, test } from "@playwright/test";

test.use({ viewport: { width: 1440, height: 1100 } });

test("invalid TURN invitation stays idle and allows creating a new room", async ({ page }) => {
  const pageErrors: string[] = [];
  const registrationRequests: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === "POST" && (path === "/devices" || path === "/rooms")) {
      registrationRequests.push(path);
    }
  });

  await page.goto("/");
  const invalidInvitation = new URL(page.url());
  invalidInvitation.hash = new URLSearchParams({
    invite: JSON.stringify({
      version: 1,
      roomId: "r_synthetic_invalid_invitation",
      roomToken: "synthetic-test-token-not-a-real-credential",
      signalingOrigin: "http://localhost:8787",
      iceServers: [{ urls: ["turn:relay.example.test:3478?transport=udp"] }],
    }),
  }).toString();

  const create = page.getByTestId("create-room");
  const join = page.getByTestId("join-room");
  const state = page.getByTestId("media-state");
  const error = page.getByRole("alert");
  await page.getByTestId("invitation-input").fill(invalidInvitation.toString());
  await join.click();

  await expect(error).toBeVisible();
  await expect(error).toContainText("TURN 설정에는 사용자 이름과 자격 증명이 필요합니다.");
  await expect(state).toHaveAttribute("data-state", "idle");
  await expect(join).toBeEnabled();
  await expect(create).toBeEnabled();
  expect(registrationRequests).toEqual([]);
  expect(pageErrors).toEqual([]);

  // Capture only the rejected synthetic invitation, before a real room/token is created.
  await mkdir("proof", { recursive: true });
  await page.screenshot({ path: "proof/invalid-invitation.png", fullPage: false });
  await expect(error).toBeVisible();
  await expect(state).toHaveAttribute("data-state", "idle");
  expect(registrationRequests).toEqual([]);

  await create.click();
  await expect(state).toHaveAttribute("data-state", "joined");
  await expect(error).toBeHidden();
  expect(registrationRequests).toEqual(["/devices", "/rooms"]);
  await page.getByTestId("leave-room").click();
  await expect(state).toHaveAttribute("data-state", "idle");
  await expect(join).toBeEnabled();
  await expect(create).toBeEnabled();
  expect(pageErrors).toEqual([]);
});
