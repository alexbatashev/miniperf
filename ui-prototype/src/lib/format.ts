export function fmtCount(n: number): string {
  if (n >= 1e12) return (n / 1e12).toFixed(2) + "T"
  if (n >= 1e9) return (n / 1e9).toFixed(2) + "G"
  if (n >= 1e6) return (n / 1e6).toFixed(1) + "M"
  if (n >= 1e3) return (n / 1e3).toFixed(1) + "K"
  return String(Math.round(n))
}

export function fmtPct(x: number, digits = 1): string {
  return (x * 100).toFixed(digits) + "%"
}

export function fmtTimeS(seconds: number): string {
  if (seconds >= 60) {
    const m = Math.floor(seconds / 60)
    return `${m}m ${(seconds - m * 60).toFixed(1)}s`
  }
  if (seconds >= 1) return seconds.toFixed(2) + "s"
  return (seconds * 1000).toFixed(0) + "ms"
}

export function fmtBytes(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(1) + " GiB"
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " MiB"
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + " KiB"
  return bytes + " B"
}
