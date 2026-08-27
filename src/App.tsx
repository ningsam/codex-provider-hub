import { AihubCard } from "./cards/AihubCard";
import { CursorPoolCard } from "./cards/CursorPoolCard";
import { GatewayCard } from "./cards/GatewayCard";
import { PickerGuardCard } from "./cards/PickerGuardCard";
import { ProvidersCard } from "./cards/ProvidersCard";
import { Sub2ApiCard } from "./cards/Sub2ApiCard";
import "./App.css";

type GlyphName =
  | "overview"
  | "usage"
  | "providers"
  | "accounts"
  | "shield"
  | "route"
  | "refresh"
  | "spark";

function Glyph({ name }: { name: GlyphName }) {
  const common = {
    width: 18,
    height: 18,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
  };

  switch (name) {
    case "overview":
      return (
        <svg {...common}>
          <rect x="3" y="3" width="7" height="7" rx="2" />
          <rect x="14" y="3" width="7" height="7" rx="2" />
          <rect x="3" y="14" width="7" height="7" rx="2" />
          <rect x="14" y="14" width="7" height="7" rx="2" />
        </svg>
      );
    case "usage":
      return (
        <svg {...common}>
          <path d="M4 17V10" />
          <path d="M10 17V5" />
          <path d="M16 17v-4" />
          <path d="M22 17V8" />
          <path d="M2 21h20" />
        </svg>
      );
    case "providers":
      return (
        <svg {...common}>
          <circle cx="6" cy="12" r="3" />
          <circle cx="18" cy="6" r="3" />
          <circle cx="18" cy="18" r="3" />
          <path d="m8.7 10.6 6.6-3.2" />
          <path d="m8.7 13.4 6.6 3.2" />
        </svg>
      );
    case "accounts":
      return (
        <svg {...common}>
          <circle cx="9" cy="8" r="3" />
          <path d="M3.5 19a5.5 5.5 0 0 1 11 0" />
          <circle cx="17.5" cy="10" r="2.5" />
          <path d="M15.5 15.5a4.5 4.5 0 0 1 5 3.5" />
        </svg>
      );
    case "shield":
      return (
        <svg {...common}>
          <path d="M12 3 5 6v5c0 4.6 2.8 8 7 10 4.2-2 7-5.4 7-10V6l-7-3Z" />
          <path d="m9.3 12 1.8 1.8 3.8-4" />
        </svg>
      );
    case "route":
      return (
        <svg {...common}>
          <circle cx="6" cy="6" r="2" />
          <circle cx="18" cy="18" r="2" />
          <path d="M8 6h3a3 3 0 0 1 3 3v6a3 3 0 0 0 3 3" />
          <path d="m15 9 3-3 3 3" />
        </svg>
      );
    case "refresh":
      return (
        <svg {...common}>
          <path d="M20 11a8 8 0 1 0-2.3 5.7" />
          <path d="M20 4v7h-7" />
        </svg>
      );
    case "spark":
      return (
        <svg {...common}>
          <path d="m12 2 1.5 5.1L18 9l-4.5 1.9L12 16l-1.5-5.1L6 9l4.5-1.9L12 2Z" />
          <path d="m19 14 .8 2.2L22 17l-2.2.8L19 20l-.8-2.2L16 17l2.2-.8L19 14Z" />
        </svg>
      );
  }
}

const navigation = [
  { href: "#overview", label: "运行状态", icon: "overview" as const },
  { href: "#usage", label: "额度用量", icon: "usage" as const },
  { href: "#providers", label: "供应商", icon: "providers" as const },
  { href: "#accounts", label: "账号池", icon: "accounts" as const },
];

const facts = [
  {
    icon: "shield" as const,
    label: "本地优先",
    value: "敏感凭据留在设备",
  },
  {
    icon: "route" as const,
    label: "统一路由",
    value: "网关与供应商一处管理",
  },
  {
    icon: "refresh" as const,
    label: "自动刷新",
    value: "状态与额度持续同步",
  },
];

export default function App() {
  return (
    <div className="app-shell">
      <div className="ambient-canvas" aria-hidden>
        <span className="ambient-orb ambient-orb-a" />
        <span className="ambient-orb ambient-orb-b" />
        <span className="ambient-orb ambient-orb-c" />
        <span className="ambient-grid" />
        <span className="ambient-noise" />
      </div>

      <div className="glass-frame">
        <header className="topbar">
          <a className="brand-lockup" href="#overview" aria-label="Codex Provider Hub 首页">
            <span className="brand-mark" aria-hidden>
              <span className="brand-mark-core">C</span>
            </span>
            <span className="brand-copy">
              <span className="brand-overline">Codex</span>
              <strong>Provider Hub</strong>
            </span>
          </a>

          <div className="topbar-meta" aria-label="应用状态">
            <span className="privacy-chip">
              <span className="privacy-dot" aria-hidden />
              Local workspace
            </span>
            <span className="topbar-divider" aria-hidden />
            <span className="topbar-caption">Private control center</span>
          </div>
        </header>

        <div className="workspace">
          <aside className="sidebar" aria-label="页面导航">
            <p className="sidebar-label">Workspace</p>
            <nav className="sidebar-nav">
              {navigation.map((item, index) => (
                <a
                  className={`nav-item ${index === 0 ? "is-active" : ""}`}
                  href={item.href}
                  key={item.href}
                >
                  <span className="nav-icon">
                    <Glyph name={item.icon} />
                  </span>
                  <span>{item.label}</span>
                  <span className="nav-index">0{index + 1}</span>
                </a>
              ))}
            </nav>

            <div className="sidebar-note">
              <span className="sidebar-note-icon">
                <Glyph name="spark" />
              </span>
              <div>
                <span>Fluid workspace</span>
                <strong>清晰掌控每条路由</strong>
              </div>
            </div>
          </aside>

          <main className="main-panel">
            <section className="hero" aria-labelledby="page-title">
              <div className="hero-copy">
                <p className="hero-kicker">
                  <span className="hero-kicker-line" aria-hidden />
                  Control center
                </p>
                <h1 id="page-title">
                  一处看清你的
                  <span> Codex 资源。</span>
                </h1>
                <p className="hero-description">
                  管理本地网关、OAuth 号池、供应商与额度，在稳定、轻盈的工作区中完成每一次切换。
                </p>
              </div>

              <div className="hero-visual" aria-hidden>
                <div className="liquid-orb">
                  <span className="liquid-orb-ring liquid-orb-ring-a" />
                  <span className="liquid-orb-ring liquid-orb-ring-b" />
                  <span className="liquid-orb-core">
                    <Glyph name="spark" />
                  </span>
                </div>
                <span className="orbit-chip orbit-chip-a">Gateway</span>
                <span className="orbit-chip orbit-chip-b">Providers</span>
                <span className="orbit-chip orbit-chip-c">Accounts</span>
              </div>
            </section>

            <section className="fact-strip" aria-label="工作区特性">
              {facts.map((fact) => (
                <article className="fact-item" key={fact.label}>
                  <span className="fact-icon">
                    <Glyph name={fact.icon} />
                  </span>
                  <span className="fact-copy">
                    <strong>{fact.label}</strong>
                    <span>{fact.value}</span>
                  </span>
                </article>
              ))}
            </section>

            <div className="board">
              <section id="overview" className="board-row board-row-controls section-anchor">
                <GatewayCard />
                <PickerGuardCard />
              </section>
              <section id="usage" className="board-row board-row-usage section-anchor">
                <Sub2ApiCard />
                <AihubCard />
              </section>
              <section id="providers" className="board-row board-row-full section-anchor">
                <ProvidersCard />
              </section>
              <section id="accounts" className="board-row board-row-full section-anchor">
                <CursorPoolCard />
              </section>
            </div>
          </main>
        </div>
      </div>
    </div>
  );
}
