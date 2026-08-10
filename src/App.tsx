import { AihubCard } from "./cards/AihubCard";
import { CursorPoolCard } from "./cards/CursorPoolCard";
import { GatewayCard } from "./cards/GatewayCard";
import { Sub2ApiCard } from "./cards/Sub2ApiCard";
import "./App.css";

export default function App() {
  return (
    <div className="app-shell">
      <div className="bg-glow" aria-hidden />
      <header className="app-header">
        <div>
          <p className="brand">Codex Provider Hub</p>
          <h1>用量与网关看板</h1>
        </div>
        <p className="header-note">Live · gateway · Sub2API · AIHub · Cursor</p>
      </header>
      <main className="card-grid">
        <GatewayCard />
        <Sub2ApiCard />
        <AihubCard />
        <CursorPoolCard />
      </main>
    </div>
  );
}
