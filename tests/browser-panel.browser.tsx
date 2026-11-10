import { useState } from "react";
import { createRoot } from "react-dom/client";
import { I18nProvider } from "../src/i18n";
import BrowserPanel from "../src/chat/BrowserPanel";
import "../src/styles.css";

function Fixture() {
  const [active, setActive] = useState(true);
  const [mounted, setMounted] = useState(true);
  const [value, setValue] = useState("");
  return <I18nProvider><button id="toggle" data-active={active} onClick={() => setActive(!active)}>Toggle</button>
    <button id="unmount" onClick={() => setMounted(!mounted)}>Toggle mount</button><p>Unique page body for browser inspection.</p><label>Choice <select aria-label="Choice"><option value="one">First</option><option value="two">Second</option></select></label>
    <div style={{ position: "absolute", right: 0, top: 50, width: 400, height: 600 }}>
      {mounted && <BrowserPanel active={active} agentName="test-agent" sessionId="test-session" />}
    </div><input aria-label="Test input" value={value} onChange={(event) => setValue(event.target.value)} /></I18nProvider>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
