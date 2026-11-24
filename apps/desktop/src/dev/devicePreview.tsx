/**
 * Device fixture (`?preview=device`, docs/superpowers/plans/2026-11-21-mobile.md): a chat with its
 * Device pane open, over in-memory devices. There is no device and so no video: the live screen
 * shows as it does while its first frame is on the way. The native checks, against a real
 * emulator, are recorded in the plan.
 *
 * `?state=` chooses what the pane opens on: `setup`, `offer`, `downloading`, `failed`, `slow`,
 * `chooser` (the default), `starting`, `live`, `stopped`, `phone` or `off`; on a Mac, `mac` (an
 * Android phone and an iPhone to start), `mac-missing` (no Xcode), `iphone-starting` (AXe on its
 * way) and `iphone` (a live simulator). `window.deviceFixture` is the stand-in itself: its
 * `calls`, `change(...)` and `watching`.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { MobileStatus } from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import {
  attachResult,
  chatSummary,
  FakeTransport,
  historyPage,
  lists,
  sessionInfo,
} from "../lib/chat/testing";
import { devicePanels, MobileStore } from "../lib/mobile";
import {
  emulator,
  FakeMobile,
  held,
  MAC_IPHONE,
  macStatus,
  mobileStatus,
  OFFER,
  phone,
  simulator,
} from "./mobileFixture";
import { withPreviewContext } from "./previewContext";

declare global {
  interface Window {
    deviceFixture?: FakeMobile;
  }
}

const CHAT = "device-fixture-chat";

function statusFor(state: string): MobileStatus {
  const base = mobileStatus();
  const none = { ...base.setup, adb: false, emulator: false, acceleration: null };
  switch (state) {
    case "setup":
      return mobileStatus({ setup: none });
    case "offer":
      return mobileStatus({ setup: { ...none, fetch: OFFER } });
    case "downloading":
      return mobileStatus({
        setup: {
          ...none,
          fetch: {
            state: "running",
            name: "Google APIs Intel x86_64 Atom System Image",
            phase: "downloading",
            received_mb: 1310.2,
            total_mb: 2250.9,
          },
        },
      });
    case "failed":
      return mobileStatus({
        setup: {
          ...none,
          fetch: {
            state: "failed",
            message:
              "Android Emulator did not match the checksum in Google's index, so it was thrown away.",
          },
        },
      });
    case "slow":
      return mobileStatus({
        setup: {
          ...base.setup,
          acceleration: {
            usable: false,
            detail: "Please ensure Windows Hypervisor Platform (WHPX) is properly installed.",
            fix: 'Open "Turn Windows features on or off", tick Windows Hypervisor Platform, then restart the computer.',
          },
        },
        devices: [emulator()],
      });
    case "starting":
      return mobileStatus({
        devices: [emulator({ state: "starting", chat_id: CHAT })],
        chats: [held(CHAT, { starting: true, live: false })],
      });
    case "live":
      return mobileStatus({
        devices: [emulator({ state: "ready", chat_id: CHAT, pid: 4242 })],
        chats: [held(CHAT)],
      });
    case "stopped":
      return mobileStatus({
        devices: [emulator({ error: "The disk is too full for the emulator." })],
        chats: [held(CHAT, { live: false, error: "The disk is too full for the emulator." })],
      });
    case "phone":
      return mobileStatus({
        devices: [emulator(), phone({ chat_id: CHAT })],
        chats: [
          held(CHAT, {
            device_id: "adb:R58M12ABCDE",
            kind: "phone",
            title: "Pixel 8",
            agent_allowed: false,
          }),
        ],
      });
    case "off":
      return mobileStatus({ enabled: false });
    case "mac":
      return macStatus({
        devices: [simulator({ state: "ready" }), emulator()],
      });
    case "mac-missing":
      return macStatus({
        setup: {
          ...base.setup,
          ios: {
            ...MAC_IPHONE,
            ready: false,
            detail: null,
            missing: "Install Xcode from the App Store and open it once, then come back.",
          },
        },
      });
    case "iphone-starting":
      return macStatus({
        setup: {
          ...base.setup,
          ios: { ...MAC_IPHONE, tool: { state: "downloading", received_mb: 4.2, total_mb: 15.8 } },
        },
        devices: [simulator({ state: "starting", chat_id: CHAT })],
        chats: [
          held(CHAT, {
            device_id: "o7_iphone_1",
            kind: "simulator",
            title: "iPhone 1",
            starting: true,
            live: false,
          }),
        ],
      });
    case "iphone":
      return macStatus({
        setup: { ...base.setup, ios: { ...MAC_IPHONE, tool: { state: "ready" } } },
        devices: [simulator({ state: "ready", chat_id: CHAT })],
        chats: [held(CHAT, { device_id: "o7_iphone_1", kind: "simulator", title: "iPhone 1" })],
      });
    default:
      return mobileStatus({
        devices: [
          emulator({ state: "ready" }),
          emulator({ id: "o7_phone_2", title: "Phone 2", chat_id: "another-chat", state: "ready" }),
          phone(),
          phone({ id: "adb:NEW1", title: "Galaxy S25", state: "unauthorized" }),
        ],
      });
  }
}

/** One stand-in for the page (StrictMode builds the state twice). */
let shared: FakeMobile | null = null;

function Preview() {
  const [fixture] = useState(() => {
    const params = new URLSearchParams(location.search);
    shared ??= new FakeMobile(statusFor(params.get("state") ?? "chooser"));
    const mobile = shared;
    window.deviceFixture = mobile;
    const chat = chatSummary(CHAT, {
      status: "ready",
      agent_id: "claude",
      agent_name: "Claude Code",
    });
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async () =>
          attachResult(chat.id, 0, historyPage(chat.id, []), {
            chat,
            session: sessionInfo({ status: "ready" }),
          }),
        lists: async () =>
          lists(1, {
            capabilities: {
              images: true,
              steer: false,
              interrupt: true,
              resume: true,
              fork: false,
              rewind: false,
              set_model: true,
              set_effort: true,
              permissions: true,
              plans: false,
              usage: false,
              limits: false,
              slash_commands: false,
              add_dirs: false,
              plan_edits: false,
              autonomy_levels: ["full_auto"],
            },
          }),
      }),
    );
    devicePanels.set(CHAT, params.get("closed") === null);
    return { chat, transport: Object.assign(transport, { mobile: new MobileStore(mobile) }) };
  });
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Device verification</h1>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          chatId={fixture.chat.id}
          listed={fixture.chat}
          transport={fixture.transport}
          agents={[]}
          storage={null}
        />
      </div>
    </main>
  );
}

export function mountDevicePreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
