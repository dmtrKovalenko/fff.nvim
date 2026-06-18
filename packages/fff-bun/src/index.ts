export { binaryExists, findBinary } from "./download";
export type {
  DbHealth,
  DirItem,
  DirSearchOptions,
  DirSearchResult,
  err,
  FileFinderApi,
  FileItem,
  GrepCursor,
  GrepMatch,
  GrepMode,
  GrepOptions,
  GrepResult,
  HealthCheck,
  InitOptions,
  Location,
  MixedItem,
  MixedSearchResult,
  MultiGrepOptions,
  ok,
  Result,
  ScanProgress,
  Score,
  SearchOptions,
  SearchResult,
} from "./fff-api";

export { FileFinder } from "./finder";
export {
  getLibExtension,
  getLibFilename,
  getNpmPackageName,
  getTriple,
} from "./platform";
