import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    // The suite spawns a stub binary and touches the filesystem; there is no DOM.
    environment: "node",
  },
});
