import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import {
  ArrowUpRightIcon,
  CopyIcon,
  EditIcon,
  ExternalBrowserIcon,
} from "../../../components/icons/AppIcons";
import type { DashboardService } from "../service.types";

export interface ServiceContextMenuState {
  service: DashboardService;
  x: number;
  y: number;
  trigger: HTMLElement;
}

interface ServiceContextMenuProps {
  state: ServiceContextMenuState;
  onClose: (restoreFocus?: boolean) => void;
  onOpen: (service: DashboardService) => void;
  onOpenInNewTab: (service: DashboardService) => void;
  onOpenInSystemBrowser: (service: DashboardService) => void;
  onCopyUrl: (service: DashboardService) => void;
  onEdit: (service: DashboardService) => void;
}

export function ServiceContextMenu({
  state,
  onClose,
  onOpen,
  onOpenInNewTab,
  onOpenInSystemBrowser,
  onCopyUrl,
  onEdit,
}: ServiceContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: state.x, top: state.y });

  useLayoutEffect(() => {
    const menu = menuRef.current;

    if (!menu) {
      return;
    }

    const rectangle = menu.getBoundingClientRect();
    const edgePadding = 10;
    setPosition({
      left: Math.max(
        edgePadding,
        Math.min(state.x, window.innerWidth - rectangle.width - edgePadding),
      ),
      top: Math.max(
        edgePadding,
        Math.min(state.y, window.innerHeight - rectangle.height - edgePadding),
      ),
    });
    menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [state.x, state.y]);

  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) {
        onClose(false);
      }
    };
    const handleViewportChange = () => onClose(false);

    document.addEventListener("pointerdown", handlePointerDown, true);
    window.addEventListener("resize", handleViewportChange);
    window.addEventListener("scroll", handleViewportChange, true);

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown, true);
      window.removeEventListener("resize", handleViewportChange);
      window.removeEventListener("scroll", handleViewportChange, true);
    };
  }, [onClose]);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose(true);
      return;
    }

    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)',
      ) ?? [],
    );
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    let nextIndex: number | null = null;

    if (event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1 + items.length) % items.length;
    } else if (event.key === "ArrowUp") {
      nextIndex = (currentIndex - 1 + items.length) % items.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = items.length - 1;
    }

    if (nextIndex !== null && items.length > 0) {
      event.preventDefault();
      items[nextIndex]?.focus();
    }
  };

  const runAction = (action: () => void) => {
    onClose(false);
    action();
  };

  return createPortal(
    <div
      ref={menuRef}
      className="service-context-menu"
      role="menu"
      aria-label={`${state.service.name} actions`}
      style={position}
      onKeyDown={handleKeyDown}
    >
      <div className="service-context-menu__heading">
        <strong>{state.service.name}</strong>
        <span>{state.service.url}</span>
      </div>
      <button
        type="button"
        role="menuitem"
        onClick={() => runAction(() => onOpen(state.service))}
      >
        <ArrowUpRightIcon width={16} height={16} />
        Open
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => runAction(() => onOpenInNewTab(state.service))}
      >
        <span className="service-context-menu__plus" aria-hidden="true">
          +
        </span>
        Open in new tab
      </button>
      <div className="service-context-menu__separator" role="separator" />
      <button
        type="button"
        role="menuitem"
        onClick={() => runAction(() => onOpenInSystemBrowser(state.service))}
      >
        <ExternalBrowserIcon width={16} height={16} />
        Open in system browser
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => runAction(() => onCopyUrl(state.service))}
      >
        <CopyIcon width={16} height={16} />
        Copy URL
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => runAction(() => onEdit(state.service))}
      >
        <EditIcon width={16} height={16} />
        Edit service
      </button>
    </div>,
    document.body,
  );
}
