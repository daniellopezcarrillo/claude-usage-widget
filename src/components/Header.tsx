import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface HeaderProps {
  onRefresh: () => void;
  refreshing: boolean;
  onOpenMenu: () => void;
  lastUpdatedAt: Date | null;
  compact?: boolean;
}

const COOLDOWN_SEC = 30;

export default function Header({ onRefresh, refreshing, onOpenMenu, lastUpdatedAt, compact }: HeaderProps) {
  const [cooldownLeft, setCooldownLeft] = useState(0);

  useEffect(() => {
    if (!lastUpdatedAt) return;
    const tick = () => {
      const elapsed = (Date.now() - lastUpdatedAt.getTime()) / 1000;
      setCooldownLeft(Math.max(0, Math.ceil(COOLDOWN_SEC - elapsed)));
    };
    tick();
    const id = setInterval(tick, 500);
    return () => clearInterval(id);
  }, [lastUpdatedAt]);

  const disabled = cooldownLeft > 0 || refreshing;

  const close = async () => {
    await getCurrentWindow().close();
  };

  // Dragging is handled by the container's single onMouseDown handler in App.tsx.
  // Don't add another startDragging() here — two calls per mousedown make Windows
  // start the window-move loop and then immediately cancel it (intermittent drag).
  return (
    <div
      data-window-drag-region="true"
      className={`flex items-center justify-between ${compact ? "px-2 py-1" : "px-3 py-2"} border-b border-border/40 select-none cursor-move`}
    >
      {!compact && <span className="text-xs font-semibold text-text">Claude Usage Widget</span>}
      {compact && <span />}
      <div className="flex items-center gap-1">
        <button
          onClick={onRefresh}
          disabled={disabled}
          title={disabled && cooldownLeft > 0 ? `Disponible en ${cooldownLeft}s` : "Actualizar"}
          className="w-6 h-6 rounded hover:bg-surface-light disabled:opacity-40 text-text-dim hover:text-text"
        >
          <span className={refreshing ? "inline-block animate-spin" : ""}>⟳</span>
        </button>
        <button
          onClick={onOpenMenu}
          title="Menú"
          className="w-6 h-6 rounded hover:bg-surface-light text-text-dim hover:text-text"
        >
          ⋯
        </button>
        <button
          onClick={close}
          title="Cerrar"
          className="w-6 h-6 rounded hover:bg-red-500/30 text-text-dim hover:text-text"
        >
          ×
        </button>
      </div>
    </div>
  );
}
