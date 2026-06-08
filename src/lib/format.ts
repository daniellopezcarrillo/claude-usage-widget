export function formatRemaining(resetsAt: string): string {
  if (!resetsAt) return "사용 시 시작";
  const diff = new Date(resetsAt).getTime() - Date.now();
  if (!Number.isFinite(diff)) return "사용 시 시작";
  if (diff <= 0) return "리셋 완료";
  const h = Math.floor(diff / 3_600_000);
  const m = Math.floor((diff % 3_600_000) / 60_000);
  if (h > 24) {
    const d = Math.floor(h / 24);
    return `${d}일 ${h % 24}시간 후 리셋`;
  }
  return h > 0 ? `${h}시간 ${m}분 후 리셋` : `${m}분 후 리셋`;
}
