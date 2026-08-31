import type { ChangeEvent, RefObject } from "react";
import {
  GridIcon,
  SearchIcon,
  SettingsIcon,
} from "../../../components/icons/AppIcons";

interface AppHeaderProps {
  searchValue: string;
  onSearchChange: (value: string) => void;
  searchInputRef: RefObject<HTMLInputElement | null>;
  isDashboardActive: boolean;
  onDashboardClick: () => void;
  onSettingsClick: () => void;
}

export function AppHeader({
  searchValue,
  onSearchChange,
  searchInputRef,
  isDashboardActive,
  onDashboardClick,
  onSettingsClick,
}: AppHeaderProps) {
  const handleChange = (event: ChangeEvent<HTMLInputElement>) => {
    onSearchChange(event.currentTarget.value);
  };

  return (
    <header className="app-header">
      <div className="brand" aria-label="Personal Hub">
        <div className="brand__mark" aria-hidden="true">
          <svg viewBox="0 0 28 28" width="26" height="26" fill="none">
            <path d="m5 12 9-7 9 7v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V12Z" />
            <path d="M10 15h8M10 19h5" />
            <circle cx="19" cy="19" r="1" fill="currentColor" stroke="none" />
          </svg>
        </div>
        <div className="brand__copy">
          <span>Personal Hub</span>
          <small>Home server</small>
        </div>
      </div>

      <nav className="primary-nav" aria-label="Primary navigation">
        <button
          className={
            isDashboardActive
              ? "primary-nav__item primary-nav__item--active"
              : "primary-nav__item"
          }
          type="button"
          onClick={onDashboardClick}
        >
          <GridIcon width={17} height={17} />
          Dashboard
        </button>
      </nav>

      <div className="app-header__actions">
        <label className="search-field">
          <SearchIcon width={17} height={17} />
          <span className="visually-hidden">Search services</span>
          <input
            ref={searchInputRef}
            type="search"
            value={searchValue}
            onChange={handleChange}
            placeholder="Search services"
            autoComplete="off"
          />
          <kbd>Ctrl K</kbd>
        </label>

        <button
          className="icon-button"
          type="button"
          aria-label="Open settings"
          title="Settings"
          onClick={onSettingsClick}
        >
          <SettingsIcon width={19} height={19} />
        </button>
      </div>
    </header>
  );
}
