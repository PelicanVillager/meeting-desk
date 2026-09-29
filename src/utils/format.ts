export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(1)} ${units[unitIndex]}`;
}

/** 去掉扩展名，用作议题名等处的默认值。 */
export function stripExtension(filename: string): string {
  const index = filename.lastIndexOf(".");
  return index > 0 ? filename.slice(0, index) : filename;
}
