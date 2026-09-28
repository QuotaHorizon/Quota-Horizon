import {
  BarChart3,
  Bot,
  FolderOpen,
  Gauge,
  ListFilter,
  Sparkles,
  PackageOpen,
  Radar,
  Server,
  Settings,
  UserRound,
} from "lucide-react";
import type { ReactNode } from "react";
import type { Translate } from "../../i18n";

export type DashboardPage =
  | "capacity"
  | "resetIntelligence"
  | "accounts"
  | "providers"
  | "tokens"
  | "skills"
  | "sessions"
  | "promptFilter"
  | "promptInjection"
  | "settings"
  | "claudeCode";

export interface DashboardNavigationProps {
  collapsed?: boolean;
  onPageChange: (page: DashboardPage) => void;
  page: DashboardPage;
  sidebarTools?: ReactNode;
  t: Translate;
  variant?: "top" | "sidebar";
  resetUpdatesCount?: number;
}

const NAVIGATION_ITEMS = [
  { page: "capacity", icon: Gauge, labelKey: "nav.capacity" },
  { page: "resetIntelligence", icon: Radar, labelKey: "nav.resetIntelligence" },
  { page: "accounts", icon: UserRound, labelKey: "nav.accounts" },
  { page: "sessions", icon: FolderOpen, labelKey: "nav.sessions" },
  { page: "promptFilter", icon: ListFilter, labelKey: "nav.systemPromptFilter" },
  { page: "promptInjection", icon: Sparkles, labelKey: "nav.systemPromptInjection" },
  { page: "providers", icon: Server, labelKey: "nav.providers" },
  { page: "claudeCode", icon: Bot, labelKey: "nav.claudeCode" },
  { page: "tokens", icon: BarChart3, labelKey: "nav.tokenUsage" },
  { page: "skills", icon: PackageOpen, labelKey: "nav.skills" },
] as const;

export function DashboardNavigation({
  collapsed = false,
  onPageChange,
  page,
  sidebarTools,
  t,
  variant = "top",
  resetUpdatesCount = 0,
}: DashboardNavigationProps) {
  const navigationButton = (item: typeof NAVIGATION_ITEMS[number] | {
    page: "settings";
    icon: typeof Settings;
    labelKey: "nav.settings";
  }) => {
    const Icon = item.icon;
    const label = t(item.labelKey);
    const updates = item.page === "resetIntelligence" ? resetUpdatesCount : 0;
    const accessibleLabel = updates ? `${label} · ${t("nav.resetUpdates")}` : label;
    return (
      <button key={item.page} className={page === item.page ? "selected" : ""}
        aria-label={collapsed || updates ? accessibleLabel : undefined} title={collapsed || updates ? accessibleLabel : undefined}
        onClick={() => onPageChange(item.page)}>
        <Icon size={19} /><span>{label}</span>
        {updates > 0 && <em className="reset-unread-badge" aria-hidden="true" />}
      </button>
    );
  };
  return (
    <nav className={variant === "sidebar" ? "sidebar-tabs" : "top-tabs"}
      aria-label={t("nav.aria")}>
      {NAVIGATION_ITEMS.map(navigationButton)}
      {variant === "sidebar" && (
        <div className="sidebar-nav-tools">
          {sidebarTools}
          {navigationButton({ page: "settings", icon: Settings, labelKey: "nav.settings" })}
        </div>
      )}
    </nav>
  );
}
