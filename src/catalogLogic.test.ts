import { describe, expect, it } from "vitest";
import {
  buildSlugById,
  catalogItemCount,
  displayName,
  folderDisplayName,
  formatPresenceLabel,
  formatPrepMessage,
  formatTrackConstitution,
  isCatalogSnapshot,
  isTrackConstitutionMessage,
  itemMatchesLibrarySearch,
  locationLooksLikeCatalog,
  normalizePathname,
  normalizeSearchText,
  normalizeSettings,
  normalizeQuizInterval,
  pathForRoute,
  routeFromPathname,
  routeFromSettings,
  slugify,
  trackSourceTooltip,
} from "./catalogLogic";
import type { AppSettings } from "./settings";

const baseSettings = (): AppSettings => ({
  wizardStep: "welcome",
  skipIntro: false,
  serverPort: 8787,
  setupComplete: false,
  tmdbApiKey: "",
  libraryCheckMinutes: 60,
  cacheMaxGb: 20,
  purgeCacheAfterWatch: false,
  jamQuizMode: false,
  jamQuizIntervalSeconds: 60,
  jamDisplaySubEn: false,
  jamDisplaySubFr: false,
});

describe("normalizeQuizInterval", () => {
  it("keeps whitelist values", () => {
    expect(normalizeQuizInterval(30)).toBe(30);
    expect(normalizeQuizInterval(60)).toBe(60);
    expect(normalizeQuizInterval(120)).toBe(120);
    expect(normalizeQuizInterval(300)).toBe(300);
    expect(normalizeQuizInterval(900)).toBe(900);
  });

  it("falls back to 60 for unknown values", () => {
    expect(normalizeQuizInterval(5)).toBe(60);
    expect(normalizeQuizInterval(90)).toBe(60);
    expect(normalizeQuizInterval("nope")).toBe(60);
    expect(normalizeQuizInterval(undefined)).toBe(60);
  });
});

describe("slugify", () => {
  it("strips accents and punctuation", () => {
    expect(slugify("Café & Thé!")).toBe("cafe-the");
  });

  it("falls back to item when empty", () => {
    expect(slugify("???")).toBe("item");
    expect(slugify("")).toBe("item");
  });
});

describe("buildSlugById", () => {
  it("uses the title slug when unique", () => {
    const map = buildSlugById(
      [{ id: "a", title: "Foundation" }],
      (i) => i.title,
    );
    expect(map.get("a")).toBe("foundation");
  });

  it("disambiguates collisions with tmdbId then numeric suffix", () => {
    const map = buildSlugById(
      [
        { id: "a", title: "Dune", tmdbId: 1 },
        { id: "b", title: "Dune", tmdbId: 2 },
        { id: "c", title: "Dune", tmdbId: 2 },
      ],
      (i) => i.title,
    );
    expect(map.get("a")).toBe("dune-1");
    expect(map.get("b")).toBe("dune-2");
    expect(map.get("c")).toBe("dune-2-2");
  });
});

describe("displayName / search", () => {
  it("prefers displayTitle", () => {
    expect(displayName({ title: "raw", displayTitle: " Affiché " })).toBe("Affiché");
    expect(displayName({ title: "raw", displayTitle: "  " })).toBe("raw");
  });

  it("matches ignoring accents and case", () => {
    expect(normalizeSearchText("Été")).toBe("ete");
    expect(
      itemMatchesLibrarySearch({ title: "Café", displayTitle: null }, "cafe"),
    ).toBe(true);
    expect(itemMatchesLibrarySearch({ title: "Dune" }, "")).toBe(true);
    expect(itemMatchesLibrarySearch({ title: "Dune" }, "matrix")).toBe(false);
  });
});

describe("folderDisplayName", () => {
  it("returns the last path segment", () => {
    expect(folderDisplayName("/Users/fuse/Movies/Series")).toBe("Series");
    expect(folderDisplayName("C:\\Media\\TV")).toBe("TV");
  });
});

describe("formatPresenceLabel", () => {
  it("shows salon libre only when the server runs", () => {
    expect(formatPresenceLabel([], true)).toBe("Salon libre");
    expect(formatPresenceLabel([], false)).toBe("Aucune lecture");
  });

  it("combines jam and solo with an escaped title", () => {
    const label = formatPresenceLabel(
      [
        { clientId: "1", mode: "jam", title: "<Bad>", playing: true },
        { clientId: "2", mode: "solo", title: null, playing: true },
      ],
      true,
    );
    expect(label).toContain("Solo + Jam");
    expect(label).toContain("&lt;Bad&gt;");
  });
});

describe("prep copy", () => {
  it("collapses prep messages", () => {
    expect(formatPrepMessage("  a \n b  ")).toBe("a b");
    expect(formatPrepMessage(null)).toBe("");
  });

  it("detects short constitution messages", () => {
    expect(isTrackConstitutionMessage("EN généré, FR manquant")).toBe(true);
    expect(isTrackConstitutionMessage("EN natif, FR natif")).toBe(true);
    expect(
      isTrackConstitutionMessage(
        "Pas de sous-titres français dans le fichier (anglais prêt).",
      ),
    ).toBe(false);
  });

  it("describes track sources", () => {
    expect(trackSourceTooltip("native")).toContain("Présent");
    expect(trackSourceTooltip("generated")).toContain("Généré");
    expect(trackSourceTooltip("missing")).toBe("");
    expect(
      formatTrackConstitution({
        status: "partial",
        video: true,
        subsEn: true,
        subsFr: false,
        enSource: "native",
        frSource: "missing",
      }),
    ).toBe("EN : Présent dans le fichier");
  });
});

describe("catalog snapshot guards", () => {
  it("accepts only Center-shaped payloads", () => {
    expect(isCatalogSnapshot({ series: [], movies: [] })).toBe(true);
    expect(isCatalogSnapshot({ series: [] })).toBe(false);
    expect(isCatalogSnapshot(null)).toBe(false);
    expect(catalogItemCount({ scannedAt: "", series: [{}, {}] as never, movies: [{}] as never })).toBe(
      3,
    );
  });
});

describe("normalizeSettings", () => {
  it("maps legacy done / tmdb steps", () => {
    const done = normalizeSettings({
      ...baseSettings(),
      wizardStep: "done" as never,
      setupComplete: false,
    });
    expect(done.setupComplete).toBe(true);
    expect(done.wizardStep).toBe("network");

    const tmdb = normalizeSettings({
      ...baseSettings(),
      wizardStep: "tmdb" as never,
    });
    expect(tmdb.wizardStep).toBe("options");
  });

  it("fills defaults for missing numbers", () => {
    const raw = {
      ...baseSettings(),
      libraryCheckMinutes: undefined as never,
      cacheMaxGb: undefined as never,
    };
    const out = normalizeSettings(raw);
    expect(out.libraryCheckMinutes).toBe(60);
    expect(out.cacheMaxGb).toBe(20);
  });
});

describe("routing", () => {
  const series = {
    id: "local:foundation",
    seasons: [{ number: 1 }, { number: 3 }],
  };
  const movie = { id: "local:matrix" };

  const findSeries = (ref: string) =>
    decodeURIComponent(ref) === "foundation" || ref === series.id ? series : undefined;
  const findMovie = (ref: string) =>
    decodeURIComponent(ref) === "the-matrix" || ref === movie.id ? movie : undefined;

  it("normalizes trailing slashes and index.html", () => {
    expect(normalizePathname("/library/")).toBe("/library");
    expect(normalizePathname("/index.html")).toBe("/");
  });

  it("parses setup, library, series, season, movie", () => {
    expect(routeFromPathname("/setup/media", findSeries, findMovie)).toEqual({
      view: "setup",
      step: "media",
    });
    expect(routeFromPathname("/setup/nope", findSeries, findMovie)).toEqual({
      view: "setup",
      step: "welcome",
    });
    expect(routeFromPathname("/library", findSeries, findMovie)).toEqual({
      view: "library",
    });
    expect(routeFromPathname("/series/foundation", findSeries, findMovie)).toEqual({
      view: "series",
      seriesId: series.id,
    });
    expect(routeFromPathname("/series/foundation/s3", findSeries, findMovie)).toEqual({
      view: "season",
      seriesId: series.id,
      season: 3,
    });
    expect(routeFromPathname("/movies/the-matrix", findSeries, findMovie)).toEqual({
      view: "movie",
      movieId: movie.id,
    });
  });

  it("falls back to library for unknown catalog refs", () => {
    expect(routeFromPathname("/series/missing", findSeries, findMovie)).toEqual({
      view: "library",
    });
    expect(routeFromPathname("/series/foundation/s9", findSeries, findMovie)).toEqual({
      view: "library",
    });
  });

  it("builds paths and detects catalog URLs", () => {
    expect(
      pathForRoute({ view: "season", seriesId: "x", season: 2 }, () => "show", () => "film"),
    ).toBe("/series/show/s2");
    expect(locationLooksLikeCatalog("/movies/x")).toBe(true);
    expect(locationLooksLikeCatalog("/setup/media")).toBe(false);
  });

  it("derives the initial route from settings", () => {
    expect(routeFromSettings(null)).toEqual({ view: "setup", step: "welcome" });
    expect(
      routeFromSettings({ ...baseSettings(), wizardStep: "options", setupComplete: false }),
    ).toEqual({ view: "setup", step: "options" });
    expect(routeFromSettings({ ...baseSettings(), setupComplete: true })).toEqual({
      view: "library",
    });
  });
});
