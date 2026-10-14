import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Pill from "./Pill";
import "./styles.css";

// Both windows load this page; the window label picks the view.
const isPill = getCurrentWindow().label === "pill";
document.documentElement.classList.add(isPill ? "pill-window" : "main-window");

createRoot(document.getElementById("root")!).render(<StrictMode>{isPill ? <Pill /> : <App />}</StrictMode>);
