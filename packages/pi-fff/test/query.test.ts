import { describe, expect, test } from "bun:test";
import { buildQuery, normalizePathConstraint } from "../src/query";

const cwd = "/tmp/workspace";

describe("path constraint normalization", () => {
  test("converts absolute in-workspace paths to repo-relative constraints", () => {
    expect(normalizePathConstraint("/tmp/workspace/.agents/**", cwd)).toBe(".agents/");
    expect(normalizePathConstraint("/tmp/workspace/.agents/plans/**", cwd)).toBe(
      ".agents/plans/",
    );
  });

  test("rejects absolute paths outside the workspace", () => {
    expect(() => normalizePathConstraint("/tmp/other/.agents/**", cwd)).toThrow(
      "Path constraint must be relative to the workspace",
    );
  });

  test("collapses only simple trailing recursive directory globs", () => {
    expect(normalizePathConstraint(".agents/**", cwd)).toBe(".agents/");
    expect(normalizePathConstraint("src/**/*", cwd)).toBe("src/");
    expect(normalizePathConstraint("src/**/*.ts", cwd)).toBe("src/**/*.ts");
    expect(normalizePathConstraint("{src,lib}/**", cwd)).toBe("{src,lib}/**");
  });

  test("builds find queries with normalized include and exclude constraints", () => {
    expect(
      buildQuery("/tmp/workspace/.agents/**", "*", "/tmp/workspace/test/**", cwd),
    ).toBe(".agents/ !test/ *");
  });
});
