export function formatRemaining(resetsAt: string): string {
  if (!resetsAt) return "Empieza al usar";
  const diff = new Date(resetsAt).getTime() - Date.now();
  if (!Number.isFinite(diff)) return "Empieza al usar";
  if (diff <= 0) return "Reseteo completado";
  const h = Math.floor(diff / 3_600_000);
  const m = Math.floor((diff % 3_600_000) / 60_000);
  if (h > 24) {
    const d = Math.floor(h / 24);
    return `${d}d ${h % 24}h para reseteo`;
  }
  return h > 0 ? `${h}h ${m}m para reseteo` : `${m}m para reseteo`;
}
