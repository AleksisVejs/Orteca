/** Windows may add this for long paths; it is not part of the folder name. */
export function visiblePath(path: string): string {
  const uncPrefix = "\\\\?\\UNC\\";
  if (path.startsWith(uncPrefix)) return `\\\\${path.slice(uncPrefix.length)}`;
  const longPrefix = "\\\\?\\";
  return path.startsWith(longPrefix) ? path.slice(longPrefix.length) : path;
}
