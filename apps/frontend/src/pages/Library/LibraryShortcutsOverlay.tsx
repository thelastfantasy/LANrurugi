import { useTranslation } from "react-i18next";

import { Keycap as Key } from "@/components/common-ui/Display/Keycap";

/** The library's counterpart to the reader's shortcut sheet: same dark OSD language, listing only
 *  the keys this page actually handles (search focus/navigation, dismissals, this very sheet). */
export function LibraryShortcutsOverlay({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();

  return (
    <>
      <div
        className="shortcut-sheet-shade"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        className="shortcut-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.shortcutsTitle") ?? undefined}
        data-shortcut-sheet
      >
        <h4 className="shortcut-sheet-title">{t("library.shortcutsTitle")}</h4>
        <div className="reader-help-sections">
          <section className="reader-help-section">
            <h5 className="reader-help-section-title">{t("library.shortcutsSearch")}</h5>
            <ul className="reader-help-list">
              <li>
                <span className="reader-help-keys">
                  <Key>/</Key>
                </span>
                <span>{t("library.shortcutsFocusSearch")}</span>
              </li>
              <li>
                <span className="reader-help-keys">
                  <Key>Enter</Key>
                </span>
                <span>{t("library.shortcutsRunSearch")}</span>
              </li>
              <li>
                <span className="reader-help-keys">
                  <Key>↑</Key>
                  <Key>↓</Key>
                </span>
                <span>{t("library.shortcutsMoveSuggestions")}</span>
              </li>
              <li>
                <span className="reader-help-keys">
                  <Key>Ctrl</Key>
                  <Key>Delete</Key>
                </span>
                <span>{t("library.shortcutsForgetRecent")}</span>
              </li>
              <li>
                <span className="reader-help-keys">
                  <Key>Esc</Key>
                </span>
                <span>{t("library.shortcutsCloseSuggestions")}</span>
              </li>
            </ul>
          </section>

          <section className="reader-help-section">
            <h5 className="reader-help-section-title">{t("library.shortcutsOther")}</h5>
            <ul className="reader-help-list">
              <li>
                <span className="reader-help-keys">
                  <Key>?</Key>
                </span>
                <span>{t("library.shortcutsToggleHelp")}</span>
              </li>
              <li>
                <span className="reader-help-keys">
                  <Key>Esc</Key>
                </span>
                <span>{t("library.shortcutsCloseMenus")}</span>
              </li>
              <li className="reader-help-plain">
                {t("library.shortcutsContextMenuHint")}
              </li>
            </ul>
          </section>
        </div>
      </div>
    </>
  );
}
