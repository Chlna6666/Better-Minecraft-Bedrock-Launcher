import { NavLink, Outlet } from "react-router-dom";
import { Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import "./ToolsLayout.css";

export default function ToolsLayout() {
  const { t } = useTranslation();

  return (
    <div className="tools-page-container">
      <div className="tools-layout">
        <aside className="tools-sidebar glass">
          <div className="tools-sidebar-title">{t("Tools.sidebar_title")}</div>
          <nav className="tools-nav">
            <NavLink
              to="/tools/online"
              className={({ isActive }) => (isActive ? "tools-nav-item active" : "tools-nav-item")}
            >
              <div className="tools-nav-icon">
                <Users size={18} />
              </div>
              <div className="tools-nav-text">
                <div className="tools-nav-name">{t("Tools.online_title")}</div>
                <div className="tools-nav-desc">{t("Tools.online_desc")}</div>
              </div>
            </NavLink>
          </nav>
        </aside>

        <main className="tools-content">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
