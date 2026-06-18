// Native library imports embedded via `bun build --compile` (type: "file").
// Each resolves to the embedded file's path string at runtime.
declare module "*.so" {
  const path: string;
  export default path;
}
declare module "*.dylib" {
  const path: string;
  export default path;
}
declare module "*.dll" {
  const path: string;
  export default path;
}

// Build-time constant injected via `bun build --define FFF_LIBC='"musl"'`.
declare const FFF_LIBC: "gnu" | "musl";
