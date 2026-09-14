export default {
  "src-tauri/**/*.rs": () => [
    "node scripts/lint-rs.mjs fmt",
    "node scripts/lint-rs.mjs clippy",
  ],
  "{src/**/*.ts,vite.config.ts}": () => [
    "npx --no-install tsc --noEmit",
    "npx --no-install oxlint --deny-warnings src vite.config.ts",
  ],
  "scripts/*.mjs": "oxlint --deny-warnings",
  "src-tauri/viewer/**/*.js": (files) => {
    const filtered = files.filter((file) => !file.endsWith(".min.js"));
    if (filtered.length === 0) {
      return [];
    }
    return [`npx --no-install oxlint --deny-warnings ${filtered.map((file) => JSON.stringify(file)).join(" ")}`];
  },
};
