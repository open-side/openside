import { useNavigate } from "react-router";
import { Composer } from "../components/Composer";
import { t } from "../i18n";
import { BrandMark } from "../ui";
import "./mini.css";

/**
 * `/mini`: the window the openside extension's mini popup command opens (480×600, no sidebar or
 * toolbar, see Layout's `data-mini`). A sent message opens its chat in the same window.
 */
export function MiniPage() {
  const navigate = useNavigate();
  return (
    <main className="mini" aria-label={t("shell.toolbar.mini")}>
      <span className="mini-mark">
        <BrandMark size={20} />
      </span>
      <Composer
        variant="home"
        onSent={(created) => {
          if (created) navigate(`/sessions/${encodeURIComponent(created)}`);
        }}
      />
    </main>
  );
}
