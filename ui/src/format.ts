/** "just now" / "5m ago" / "3h ago" / "2d ago" for candidate rows. */
export function relativeAge(nowUnixMs: number, capturedAtUnixMs: number): string {
    const minutes = Math.floor((nowUnixMs - capturedAtUnixMs) / 60000);
    if (minutes < 1) return "just now";
    if (minutes < 60) return `${minutes}m ago`;
    if (minutes < 60 * 24) return `${Math.floor(minutes / 60)}h ago`;
    return `${Math.floor(minutes / (60 * 24))}d ago`;
}

/** 0.0-1.0 score to a percent label. */
export function scorePercent(score: number): string {
    return `${Math.floor(score * 100)}%`;
}
