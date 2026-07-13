import React from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { App } from "./App";
import "./styles.css";

// Webview console output is invisible in a headless launch; ferry errors to
// the shell's stderr so failures are diagnosable from logs.
const shellLog = (message: string) => invoke("ui_log", { message }).catch(() => {});
window.addEventListener("error", (e) => shellLog(`js error: ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => shellLog(`unhandled rejection: ${e.reason}`));
shellLog("frontend mounted");

createRoot(document.getElementById("root")!).render(React.createElement(App));
