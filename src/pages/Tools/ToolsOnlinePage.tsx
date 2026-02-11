import { useTranslation } from "react-i18next";
import OnlinePanel from "../Online/OnlinePanel";

export default function ToolsOnlinePage() {
  const { t } = useTranslation();

  return (
    <div className="tools-main-panel bm-anim-page-in">
      <div className="tools-compact-header">
        <div className="tools-header-top-row">
          <div className="tools-header-info">
            <h2>{t("Online.title")}</h2>
          </div>
        </div>
      </div>
      <div className="tools-body-content">
        <OnlinePanel />
      </div>
    </div>
  );
}
