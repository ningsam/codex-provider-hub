import { useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AihubCard } from "./cards/AihubCard";
import { CursorPoolCard } from "./cards/CursorPoolCard";
import { GatewayCard } from "./cards/GatewayCard";
import { PickerGuardCard } from "./cards/PickerGuardCard";
import { ProvidersCard } from "./cards/ProvidersCard";
import { Sub2ApiCard } from "./cards/Sub2ApiCard";
import { localeOptions, useI18n, type TranslationKey } from "./i18n";
import "./App.css";

type Theme = "dark" | "light";
type GlyphName =
  | "overview"
  | "usage"
  | "providers"
  | "accounts"
  | "shield"
  | "route"
  | "refresh"
  | "spark"
  | "sun"
  | "moon"
  | "hide"
  | "command";

function Glyph({ name, size = 18 }: { name: GlyphName; size?: number }) {
  const common = {
    width: size,
    height: size,
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
          <path d="M4 18V11" />
          <path d="M10 18V5" />
          <path d="M16 18v-4" />
          <path d="M22 18V8" />
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
    case "sun":
      return (
        <svg {...common}>
          <circle cx="12" cy="12" r="3.5" />
          <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
        </svg>
      );
    case "moon":
      return (
        <svg {...common}>
          <path d="M20.4 14.2A8 8 0 0 1 9.8 3.6 8.2 8.2 0 1 0 20.4 14.2Z" />
        </svg>
      );
    case "hide":
      return (
        <svg {...common}>
          <path d="M5 12h14" />
        </svg>
      );
    case "command":
      return (
        <svg {...common}>
          <path d="M9 6V5a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v14a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6Z" />
        </svg>
      );
  }
}

const navigation: Array<{
  id: string;
  labelKey: TranslationKey;
  metaKey: TranslationKey;
  icon: GlyphName;
}> = [
  { id: "overview", labelKey: "nav.status", metaKey: "nav.overview", icon: "overview" },
  { id: "usage", labelKey: "nav.usage", metaKey: "nav.usage", icon: "usage" },
  { id: "providers", labelKey: "nav.providers", metaKey: "nav.routes", icon: "providers" },
  { id: "accounts", labelKey: "nav.accounts", metaKey: "nav.accounts", icon: "accounts" },
];

const principles: Array<{
  icon: GlyphName;
  labelKey: TranslationKey;
  valueKey: TranslationKey;
}> = [
  {
    icon: "shield",
    labelKey: "principles.localFirst",
    valueKey: "principles.localFirstValue",
  },
  {
    icon: "route",
    labelKey: "principles.oneRoute",
    valueKey: "principles.oneRouteValue",
  },
  {
    icon: "refresh",
    labelKey: "principles.liveState",
    valueKey: "principles.liveStateValue",
  },
];

function initialTheme(): Theme {
  const saved = window.localStorage.getItem("codex-hub-theme");
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export default function App() {
  const { locale, setLocale, t } = useI18n();
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [activeSection, setActiveSection] = useState("overview");

  const localizedNavigation = useMemo(
    () =>
      navigation.map((item) => ({
        ...item,
        label: t(item.labelKey),
        meta: t(item.metaKey),
      })),
    [t],
  );

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem("codex-hub-theme", theme);
  }, [theme]);

  useEffect(() => {
    const root = document.querySelector<HTMLElement>(".main-panel");
    const sections = navigation
      .map((item) => document.getElementById(item.id))
      .filter((section): section is HTMLElement => Boolean(section));

    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
        if (visible?.target.id) setActiveSection(visible.target.id);
      },
      { root, rootMargin: "-12% 0px -68%", threshold: [0.08, 0.25, 0.55] },
    );

    sections.forEach((section) => observer.observe(section));
    return () => observer.disconnect();
  }, []);

  const hideWindow = () => {
    void getCurrentWindow().hide();
  };

  return (
    <div className="app-shell">
      <div className="ambient-canvas" aria-hidden>
        <span className="ambient-aurora ambient-aurora-a" />
        <span className="ambient-aurora ambient-aurora-b" />
        <span className="ambient-aurora ambient-aurora-c" />
        <span className="ambient-noise" />
      </div>

      <div className="glass-frame">
        <span className="frame-specular" aria-hidden />
        <span className="frame-refraction" aria-hidden />

        <header className="topbar" data-tauri-drag-region>
          <a className="brand-lockup" href="#overview" aria-label={t("app.home")}>
            <span className="brand-mark" aria-hidden>
              <span className="brand-mark-glow" />
              <span className="brand-mark-core">C</span>
            </span>
            <span className="brand-copy">
              <strong>Codex Provider Hub</strong>
              <span>Local control plane</span>
            </span>
          </a>

          <div className="topbar-status" aria-label={t("app.workspaceStatus")}>
            <span className="live-chip">
              <span className="live-dot" aria-hidden />
              {t("app.liveWorkspace")}
            </span>
            <span className="topbar-separator" aria-hidden />
            <span className="topbar-caption">{t("app.privateOnDevice")}</span>
          </div>

          <div className="window-actions">
            <div className="language-switcher" aria-label={t("language.label")}>
              {localeOptions.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={`language-action ${locale === option.value ? "is-active" : ""}`}
                  onClick={() => setLocale(option.value)}
                  aria-label={t(option.labelKey)}
                  title={t(option.labelKey)}
                >
                  {option.short}
                </button>
              ))}
            </div>
            <button
              type="button"
              className="window-action"
              onClick={() => setTheme((value) => (value === "dark" ? "light" : "dark"))}
              aria-label={theme === "dark" ? t("app.switchLight") : t("app.switchDark")}
              title={theme === "dark" ? t("app.lightTheme") : t("app.darkTheme")}
            >
              <Glyph name={theme === "dark" ? "sun" : "moon"} size={16} />
            </button>
            <button
              type="button"
              className="window-action"
              onClick={hideWindow}
              aria-label={t("app.hideConsole")}
              title={t("app.hide")}
            >
              <Glyph name="hide" size={16} />
            </button>
          </div>
        </header>

        <div className="workspace">
          <aside className="sidebar" aria-label={t("app.navigation")}>
            <p className="sidebar-label">{t("app.workspace")}</p>
            <nav className="sidebar-nav">
              {localizedNavigation.map((item, index) => (
                <a
                  className={`nav-item ${activeSection === item.id ? "is-active" : ""}`}
                  href={`#${item.id}`}
                  key={item.id}
                  onClick={() => setActiveSection(item.id)}
                >
                  <span className="nav-icon">
                    <Glyph name={item.icon} />
                  </span>
                  <span className="nav-copy">
                    <strong>{item.label}</strong>
                    <span>{item.meta}</span>
                  </span>
                  <span className="nav-index">0{index + 1}</span>
                </a>
              ))}
            </nav>

            <div className="sidebar-status">
              <span className="sidebar-status-icon">
                <Glyph name="command" size={16} />
              </span>
              <div>
                <strong>{t("app.localRuntime")}</strong>
                <span>127.0.0.1:18080</span>
              </div>
            </div>
          </aside>

          <main className="main-panel">
            <section className="command-header" aria-labelledby="page-title">
              <div className="command-copy">
                <p className="command-kicker">
                  <span aria-hidden />
                  {t("hero.kicker")}
                </p>
                <h1 id="page-title">
                  {t("hero.line1")}
                  <span>{t("hero.line2")}</span>
                </h1>
                <p>{t("hero.body")}</p>
                <div className="command-tags" aria-label={t("hero.features")}>
                  <span>LOCAL ONLY</span>
                  <span>ENCRYPTED</span>
                  <span>AUTO REFRESH</span>
                </div>
              </div>

              <div className="liquid-lens" aria-hidden>
                <span className="liquid-lens-halo" />
                <span className="liquid-lens-ring liquid-lens-ring-a" />
                <span className="liquid-lens-ring liquid-lens-ring-b" />
                <span className="liquid-lens-core">
                  <Glyph name="spark" size={22} />
                </span>
                <span className="lens-node lens-node-a">Gateway</span>
                <span className="lens-node lens-node-b">OAuth</span>
                <span className="lens-node lens-node-c">Catalog</span>
              </div>
            </section>

            <section className="principle-strip" aria-label={t("principles.label")}>
              {principles.map((item) => (
                <article className="principle-item" key={item.labelKey}>
                  <span className="principle-icon">
                    <Glyph name={item.icon} size={17} />
                  </span>
                  <span>
                    <strong>{t(item.labelKey)}</strong>
                    <small>{t(item.valueKey)}</small>
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
