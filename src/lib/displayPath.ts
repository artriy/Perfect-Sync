const WINDOWS_VERBATIM_UNC_PREFIX = "\\\\?\\UNC\\";
const WINDOWS_VERBATIM_PREFIX = "\\\\?\\";
const SLASHED_VERBATIM_UNC_PREFIX = "//?/UNC/";
const SLASHED_VERBATIM_PREFIX = "//?/";

/** Remove Windows verbatim-path syntax from user-facing labels without changing stored paths. */
export function displayPath(path: string): string {
  let value = path;
  while (true) {
    const folded = value.toUpperCase();
    if (folded.startsWith(WINDOWS_VERBATIM_UNC_PREFIX.toUpperCase())) {
      return `\\\\${value.slice(WINDOWS_VERBATIM_UNC_PREFIX.length)}`;
    }
    if (value.startsWith(WINDOWS_VERBATIM_PREFIX)) {
      value = value.slice(WINDOWS_VERBATIM_PREFIX.length);
      continue;
    }
    if (folded.startsWith(SLASHED_VERBATIM_UNC_PREFIX.toUpperCase())) {
      return `//${value.slice(SLASHED_VERBATIM_UNC_PREFIX.length)}`;
    }
    if (value.startsWith(SLASHED_VERBATIM_PREFIX)) {
      value = value.slice(SLASHED_VERBATIM_PREFIX.length);
      continue;
    }
    return value;
  }
}
