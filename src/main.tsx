import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installPreview } from "./preview";
import "./styles.css";

// `?preview=<state>` renders the real component tree against a fake snapshot, in a plain
// browser, with no build and no running core. Must run before React mounts.
installPreview();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
