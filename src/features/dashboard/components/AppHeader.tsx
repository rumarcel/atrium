import type { ChangeEvent, RefObject } from "react";
import { SearchIcon, SettingsIcon } from "../../../components/icons/AppIcons";
import { useTranslation } from "../../i18n";

interface AppHeaderProps {
  searchValue: string;
  onSearchChange: (value: string) => void;
  searchInputRef: RefObject<HTMLInputElement | null>;
  onSettingsClick: () => void;
}

export function AppHeader({
  searchValue,
  onSearchChange,
  searchInputRef,
  onSettingsClick,
}: AppHeaderProps) {
  const { t } = useTranslation();
  const handleChange = (event: ChangeEvent<HTMLInputElement>) => {
    onSearchChange(event.currentTarget.value);
  };

  return (
    <header className="app-header">
      <div className="brand" aria-label={t("header.brandLabel")}>
        <div className="brand__mark" aria-hidden="true">
          <svg viewBox="0 0 28 28" width="26" height="26" fill="none">
            <path d="m5 12 9-7 9 7v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V12Z" />
            <path d="M10 15h8M10 19h5" />
            <circle cx="19" cy="19" r="1" fill="currentColor" stroke="none" />
          </svg>
        </div>
        <div className="brand__copy">
          <span>{t("app.name")}</span>
          <small>{t("header.brandSubtitle")}</small>
        </div>
      </div>

      <div className="app-header__actions">
        <label className="search-field">
          <SearchIcon width={17} height={17} />
          <span className="visually-hidden">{t("header.searchServices")}</span>
          <input
            ref={searchInputRef}
            type="search"
            value={searchValue}
            onChange={handleChange}
            placeholder={t("header.searchServices")}
            autoComplete="off"
          />
          <kbd>{t("header.searchShortcut")}</kbd>
        </label>

        <button
          className="icon-button"
          type="button"
          aria-label={t("header.openSettings")}
          title={t("header.settingsTitle")}
          onClick={onSettingsClick}
        >
          <SettingsIcon width={19} height={19} />
        </button>
      </div>
    </header>
  );
}
