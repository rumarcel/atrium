import type { ComponentType, SVGProps } from "react";
import {
  ChevronRightIcon,
  DownloadIcon,
  GridIcon,
  HomeIcon,
  PlayIcon,
  ServerIcon,
  SettingsIcon,
} from "../components/icons/AppIcons";
import { useTranslation } from "../features/i18n";
import type { VerdictTone } from "./ServerVerdict";

export type ShellView = "home" | "services" | "media" | "downloads" | "servers";

interface SidebarProps {
  view: ShellView;
  onNavigate: (view: ShellView) => void;
  onOpenSettings: () => void;
  verdict: { tone: VerdictTone; text: string };
  onlineCount: number;
  totalCount: number;
}

const ITEMS: readonly {
  view: ShellView;
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  labelKey:
    | "nav.home"
    | "nav.services"
    | "nav.media"
    | "nav.downloads"
    | "nav.servers";
}[] = [
  { view: "home", icon: HomeIcon, labelKey: "nav.home" },
  { view: "services", icon: GridIcon, labelKey: "nav.services" },
  { view: "media", icon: PlayIcon, labelKey: "nav.media" },
  { view: "downloads", icon: DownloadIcon, labelKey: "nav.downloads" },
  { view: "servers", icon: ServerIcon, labelKey: "nav.servers" },
];

/**
 * A rail rather than a tab strip: the destinations are stable, so they stay in
 * one place instead of appearing and disappearing as views are opened.
 *
 * It ends with the single fact worth knowing without opening anything, which
 * is why the status sits at the bottom and not among the destinations.
 */
export function Sidebar({
  view,
  onNavigate,
  onOpenSettings,
  verdict,
  onlineCount,
  totalCount,
}: SidebarProps) {
  const { number, t } = useTranslation();

  return (
    <aside className="rail">
      <div className="rail__brand">
        <span className="rail__mark" aria-hidden="true">
          <HomeIcon width={22} height={22} />
        </span>
        <span className="rail__name">
          <strong>{t("app.name")}</strong>
          <span>{t("nav.tagline")}</span>
        </span>
      </div>

      <nav className="rail__nav" aria-label={t("nav.label")}>
        {ITEMS.map((item) => {
          const Icon = item.icon;
          return (
            <button
              type="button"
              key={item.view}
              className={
                view === item.view ? "rail__item rail__item--active" : "rail__item"
              }
              aria-current={view === item.view ? "page" : undefined}
              onClick={() => onNavigate(item.view)}
            >
              <Icon width={19} height={19} />
              {t(item.labelKey)}
            </button>
          );
        })}

        <button type="button" className="rail__item" onClick={onOpenSettings}>
          <SettingsIcon width={19} height={19} />
          {t("nav.settings")}
        </button>
      </nav>

      <button
        type="button"
        className={`rail__status rail__status--${verdict.tone}`}
        onClick={() => onNavigate("servers")}
      >
        <span className={`dot dot--${verdict.tone}`} aria-hidden="true" />
        <span className="rail__status-copy">
          <strong>{verdict.text}</strong>
          <span>
            {t("nav.onlineCount", {
              online: number(onlineCount),
              total: number(totalCount),
            })}
          </span>
        </span>
        <ChevronRightIcon width={15} height={15} />
      </button>
    </aside>
  );
}
