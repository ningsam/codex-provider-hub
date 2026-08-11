import { AihubCard } from "./cards/AihubCard";
import { ChannelSwitchCard } from "./cards/ChannelSwitchCard";
import { CodexSessionsCard } from "./cards/CodexSessionsCard";
import { CursorPoolCard } from "./cards/CursorPoolCard";
import { GatewayCard } from "./cards/GatewayCard";
import { PickerGuardCard } from "./cards/PickerGuardCard";
import { ProvidersCard } from "./cards/ProvidersCard";
import { RouteDoctorCard } from "./cards/RouteDoctorCard";
import { Sub2ApiCard } from "./cards/Sub2ApiCard";
import "./App.css";

export default function App() {
  return (
    <div className="app-shell">
      <div className="bg-atmosphere" aria-hidden>
        <span className="bg-glow bg-glow-a" />
        <span className="bg-glow bg-glow-b" />
        <span className="bg-grain" />
      </div>

      <header className="app-header">
        <div className="header-brand">
          <p className="brand-eyebrow">Provider control</p>
          <h1 className="brand-title">Codex Provider Hub</h1>
          <p className="brand-line">
            本地网关 · 号池 · 供应商 · 用量，一条清晰的控制台。
          </p>
        </div>
        <div className="header-meta">
          <span className="live-dot" aria-hidden />
          <span>Live</span>
          <span className="meta-sep">/</span>
          <span>gateway · pool · providers</span>
        </div>
      </header>

      <main className="board">
        <div className="board-row board-row-controls">
          <GatewayCard />
          <PickerGuardCard />
        </div>
        <div className="board-row board-row-usage">
          <Sub2ApiCard />
          <AihubCard />
        </div>
        <div className="board-row board-row-full">
          <RouteDoctorCard />
        </div>
        <div className="board-row board-row-full">
          <ProvidersCard />
        </div>
        <div className="board-row board-row-full">
          <ChannelSwitchCard />
        </div>
        <div className="board-row board-row-full">
          <CursorPoolCard />
        </div>
        <div className="board-row board-row-full">
          <CodexSessionsCard />
        </div>
      </main>
    </div>
  );
}
