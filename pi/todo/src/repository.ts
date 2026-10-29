/**
 * Canonical origin identity, independent of clone transport and checkout path.
 * @param {string} origin Git's origin URL.
 * @returns {string} Lowercase host and case-preserved repository path.
 */
const repositoryIdentity = (origin: string): string => {
  const value = origin.trim(),
    scp = value.includes("://")
      ? null
      : /^(?:[^/@:\s]+@)?(?<host>[^/:\s]+):(?<path>[^\s]+)$/u.exec(value),
    url = new URL(
      scp?.groups === undefined ? value : `ssh://${scp.groups["host"]}/${scp.groups["path"]}`,
    ),
    path = url.pathname
      .replace(/^\//u, "")
      .replace(/\/$/u, "")
      .replace(/\.git$/u, "");
  if (
    !["ssh:", "https:", "http:", "git:"].includes(url.protocol) ||
    url.hostname === "" ||
    url.search !== "" ||
    url.hash !== "" ||
    path === "" ||
    path.split("/").some((part) => part === "" || part === "." || part === "..")
  ) {
    throw new Error("IMPORT_FAILED: origin must identify a remote repository host/path");
  }
  return `${url.hostname.toLowerCase()}/${path}`;
};

export { repositoryIdentity };
