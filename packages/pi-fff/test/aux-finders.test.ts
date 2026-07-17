import { afterAll, describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  isOutsideWorkspaceRelativePath,
  resolveAuxRoot,
  rootCovers,
  routePathConstraint,
} from "../src/aux-finders";

describe("routePathConstraint", () => {
  const cwd = "/tmp/workspace";

  test("returns null for workspace-relative paths", () => {
    expect(routePathConstraint("src/", cwd)).toBeNull();
    expect(routePathConstraint("**/*.ts", cwd)).toBeNull();
    expect(routePathConstraint(undefined, cwd)).toBeNull();
    // Dot-prefixed names are not parent escapes.
    expect(routePathConstraint("..foo/bar", cwd)).toBeNull();
  });

  test("returns null for absolute paths inside the workspace", () => {
    expect(routePathConstraint("/tmp/workspace/src", cwd)).toBeNull();
  });

  test("routes absolute paths outside the workspace", () => {
    const route = routePathConstraint("/tmp", cwd);
    expect(route).toEqual({ root: "/tmp", suffix: "" });
  });

  if (process.platform === "win32") {
    test("treats a cross-volume relative result as outside the workspace", () => {
      const rel = path.win32.relative("D:\\workspace", "C:\\target");
      expect(rel).toBe("C:\\target");
      expect(isOutsideWorkspaceRelativePath(rel)).toBe(true);
    });
  }

  test("splits glob suffix from existing dir prefix", () => {
    const route = routePathConstraint("/tmp/**/*.ts", cwd);
    expect(route).toEqual({ root: "/tmp", suffix: "**/*.ts" });
  });

  test("walks up to existing ancestor when tail does not exist", () => {
    const route = routePathConstraint("/tmp/__nonexistent_dir_xyz__", cwd);
    expect(route).toEqual({ root: "/tmp", suffix: "__nonexistent_dir_xyz__" });
  });

  test("expands ~ to the home directory", () => {
    expect(routePathConstraint("~", cwd)).toEqual({
      root: os.homedir(),
      suffix: "",
    });
    expect(routePathConstraint("~/__fff_nope__", cwd)).toEqual({
      root: os.homedir(),
      suffix: "__fff_nope__",
    });
  });

  describe("relative paths escaping the workspace", () => {
    const base = fs.mkdtempSync(path.join(os.tmpdir(), "fff-route-"));
    const workspace = path.join(base, "workspace");
    const sibling = path.join(base, "fff-demo");
    fs.mkdirSync(workspace);
    fs.mkdirSync(sibling);

    afterAll(() => {
      fs.rmSync(base, { recursive: true, force: true });
    });

    test("routes ../sibling to the sibling directory", () => {
      expect(routePathConstraint("../fff-demo/", workspace)).toEqual({
        root: sibling,
        suffix: "",
      });
    });

    test("routes .. to the parent directory", () => {
      expect(routePathConstraint("..", workspace)).toEqual({
        root: base,
        suffix: "",
      });
    });

    test("keeps globs in the suffix", () => {
      expect(routePathConstraint("../fff-demo/**/*.ts", workspace)).toEqual({
        root: sibling,
        suffix: "**/*.ts",
      });
    });

    test("walks up when the sibling does not exist", () => {
      expect(routePathConstraint("../missing-xyz/src", workspace)).toEqual({
        root: base,
        suffix: "missing-xyz/src",
      });
    });

    test("returns null when .. resolves back inside the workspace", () => {
      expect(
        routePathConstraint("../workspace/src", workspace),
      ).toBeNull();
    });
  });
});

describe("rootCovers", () => {
  test("covers itself and descendants only", () => {
    expect(rootCovers("/a/b", "/a/b")).toBe(true);
    expect(rootCovers("/a/b", "/a/b/c")).toBe(true);
    expect(rootCovers("/", "/a")).toBe(true);
    expect(rootCovers("/a/b", "/a")).toBe(false);
    expect(rootCovers("/a/b", "/a/bc")).toBe(false);
  });
});

describe("resolveAuxRoot", () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "fff-aux-"));
  const tmpFile = path.join(tmpDir, "real-file.ts");
  fs.writeFileSync(tmpFile, "");

  afterAll(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test("returns null for relative input", () => {
    expect(resolveAuxRoot("src/")).toBeNull();
  });

  test("existing directory becomes the root with empty suffix", () => {
    expect(resolveAuxRoot(tmpDir)).toEqual({ root: tmpDir, suffix: "" });
  });

  test("existing file roots at its parent with basename suffix", () => {
    expect(resolveAuxRoot(tmpFile)).toEqual({
      root: tmpDir,
      suffix: "real-file.ts",
    });
  });

  test("nonexistent tail becomes a fuzzy suffix", () => {
    expect(resolveAuxRoot(path.join(tmpDir, "nope", "foo.ts"))).toEqual({
      root: tmpDir,
      suffix: "nope/foo.ts",
    });
  });

  test("glob after nonexistent segment stays in the suffix", () => {
    expect(resolveAuxRoot(path.join(tmpDir, "nope", "**", "*.ts"))).toEqual({
      root: tmpDir,
      suffix: "nope/**/*.ts",
    });
  });

  test("fully bogus path walks up to filesystem root", () => {
    expect(resolveAuxRoot("/__fff_nope__/x/y.ts")).toEqual({
      root: "/",
      suffix: "__fff_nope__/x/y.ts",
    });
  });

  test("bare filesystem root resolves to itself", () => {
    expect(resolveAuxRoot("/")).toEqual({ root: "/", suffix: "" });
  });

  test("trailing slashes are ignored", () => {
    expect(resolveAuxRoot(`${tmpDir}/`)).toEqual({ root: tmpDir, suffix: "" });
  });
});
