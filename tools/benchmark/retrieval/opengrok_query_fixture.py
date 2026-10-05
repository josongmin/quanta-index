"""Recreate the pinned OpenGrok 1.14.18 query-reader fixture outside the checkout.

The live capture does not import this build helper. The checked-in escaped patch is
byte-exact; the caller supplies and validates the upstream original source.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

ORIGINAL_SOURCE_SHA256 = "e7880011219a11e8b2e133c8c8b6cc678b9d8b39cc4fbb7e98836c4631c55654"
PATCHED_SOURCE_SHA256 = "21b0cb48cd58553a2d4657b1434ee5fb5228dc630f30de98ccaddd813b560f27"
SOURCE_PATCH_SHA256 = "8e6b4cdb7d11fad1fe864eb69f4a5647fa04482f77059b703fba1be2e013ca98"
COMPILER_IMAGE_ID = "sha256:1b79b7700154fec76b32816c560b1d67f30e115868fc8caf5c123207ae6074e7"
CLASS_SHA256 = {
    "org/opengrok/web/api/v1/controller/SearchController$SearchEngineWrapper.class":
        "e477af140507338e72c22eb0753720a1f63c89aa7811a7fbea2fdb67259c1335",
    "org/opengrok/web/api/v1/controller/SearchController$SearchHit.class":
        "f8945465238c3ed984f51edb987339ab85ed615ee0e7f375ac600c4780f16c65",
    "org/opengrok/web/api/v1/controller/SearchController$SearchResult.class":
        "8db685df4821fba6f3fb0cc218b78ca7fcc638fb5c72fc4845f1f42aee80ac82",
    "org/opengrok/web/api/v1/controller/SearchController.class":
        "c492587703599e7203d17872326fe8d0af694d2547b801dbe7195a4036b9c372",
}

# One escaped bytes literal per source-patch line preserves context whitespace.
PATCH_LINES = (
    b'--- a/SearchController.java\n',
    b'+++ b/SearchController.java\n',
    b'@@ -25,6 +25,7 @@\n',
    b' \n',
    b' import jakarta.inject.Inject;\n',
    b' import jakarta.servlet.http.HttpServletRequest;\n',
    b'+import jakarta.servlet.http.HttpServletResponse;\n',
    b' import jakarta.ws.rs.DefaultValue;\n',
    b' import jakarta.ws.rs.GET;\n',
    b' import jakarta.ws.rs.Path;\n',
    b'@@ -35,6 +36,11 @@\n',
    b' import jakarta.ws.rs.core.MediaType;\n',
    b' import jakarta.ws.rs.core.Response;\n',
    b' import org.apache.lucene.search.Query;\n',
    b'+import org.apache.lucene.index.DirectoryReader;\n',
    b'+import org.apache.lucene.index.IndexCommit;\n',
    b'+import org.apache.lucene.index.IndexReader;\n',
    b'+import org.apache.lucene.index.IndexReaderContext;\n',
    b'+import org.apache.lucene.index.MultiReader;\n',
    b' import org.opengrok.indexer.configuration.Project;\n',
    b' import org.opengrok.indexer.configuration.RuntimeEnvironment;\n',
    b' import org.opengrok.indexer.search.Hit;\n',
    b'@@ -45,14 +51,21 @@\n',
    b' import org.opengrok.web.api.v1.filter.CorsEnable;\n',
    b' import org.opengrok.web.api.v1.suggester.provider.service.SuggesterService;\n',
    b' \n',
    b'+import java.io.IOException;\n',
    b'+import java.nio.charset.StandardCharsets;\n',
    b'+import java.security.MessageDigest;\n',
    b'+import java.security.NoSuchAlgorithmException;\n',
    b' import java.time.Duration;\n',
    b' import java.time.Instant;\n',
    b' import java.util.ArrayList;\n',
    b' import java.util.Collections;\n',
    b'+import java.util.HexFormat;\n',
    b' import java.util.LinkedHashMap;\n',
    b' import java.util.List;\n',
    b' import java.util.Map;\n',
    b' import java.util.Set;\n',
    b'+import java.util.SortedSet;\n',
    b'+import java.util.TreeSet;\n',
    b' import java.util.stream.Collectors;\n',
    b' \n',
    b' @Path(SearchController.PATH)\n',
    b'@@ -61,8 +74,12 @@\n',
    b'     public static final String PATH = "search";\n',
    b' \n',
    b'     private static final String DEFAULT_SORT_ORDER = "relevancy";\n',
    b'+    private static final String QI_NONCE_HEADER = "X-QI-Request-Nonce";\n',
    b'+    private static final String QI_READER_HEADER = "X-QI-Reader-Witness";\n',
    b' \n',
    b'     private final SuggesterService suggester;\n',
    b'+    @Context\n',
    b'+    private HttpServletResponse response;\n',
    b' \n',
    b'     @Inject\n',
    b'     public SearchController(SuggesterService suggester) {\n',
    b'@@ -123,8 +140,17 @@\n',
    b'                     .stream()\n',
    b'                     .collect(Collectors.groupingBy(Hit::getPath,\n',
    b'                             LinkedHashMap::new,\n',
    b'-                            Collectors.mapping(h -> new SearchHit(h.getLine(), h.getLineno(), h.getTag()),\n',
    b'-                                    Collectors.toList())));\n',
    b'+                             Collectors.mapping(h -> new SearchHit(h.getLine(), h.getLineno(), h.getTag()),\n',
    b'+                                     Collectors.toList())));\n',
    b'+\n',
    b'+            // The searcher is still acquired here. Read its actual subreaders, never reopen the disk index.\n',
    b'+            String nonce = req.getHeader(QI_NONCE_HEADER);\n',
    b'+            if (nonce != null) {\n',
    b'+                if (response == null) {\n',
    b'+                    throw new WebApplicationException("Missing response context", Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+                }\n',
    b'+                response.setHeader(QI_READER_HEADER, engine.readerWitness(nonce));\n',
    b'+            }\n',
    b' \n',
    b'             long duration = Duration.between(startTime, Instant.now()).toMillis();\n',
    b' \n',
    b'@@ -140,6 +166,8 @@\n',
    b'         private final SearchEngine engine;\n',
    b' \n',
    b'         private int numResults;\n',
    b'+        private SortedSet<String> selectedProjectNames;\n',
    b'+        private int selectedProjectCount;\n',
    b' \n',
    b'         private SearchEngineWrapper(\n',
    b'                 final String full,\n',
    b'@@ -170,14 +198,18 @@\n',
    b'                 final int maxResults\n',
    b'         ) {\n',
    b'             Set<Project> allProjects = PageConfig.get(req).getProjectHelper().getAllProjects();\n',
    b'-            int collected;\n',
    b'+            List<Project> selectedProjects;\n',
    b'             if (projects == null || projects.isEmpty()) {\n',
    b'-                collected = engine.search(new ArrayList<>(allProjects));\n',
    b'+                selectedProjects = new ArrayList<>(allProjects);\n',
    b'             } else {\n',
    b'-                collected = engine.search(allProjects.stream()\n',
    b'+                selectedProjects = allProjects.stream()\n',
    b'                         .filter(p -> projects.contains(p.getName()))\n',
    b'-                        .collect(Collectors.toList()));\n',
    b'-            }\n',
    b'+                        .collect(Collectors.toList());\n',
    b'+            }\n',
    b'+            selectedProjectNames = selectedProjects.stream().map(Project::getName)\n',
    b'+                    .collect(Collectors.toCollection(TreeSet::new));\n',
    b'+            selectedProjectCount = selectedProjects.size();\n',
    b'+            int collected = engine.search(selectedProjects);\n',
    b'             numResults = engine.getTotalHits();\n',
    b' \n',
    b'             if (startDocIndex >= collected) {\n',
    b'@@ -193,6 +225,54 @@\n',
    b'             engine.results(startDocIndex, startDocIndex + resultSize, results);\n',
    b' \n',
    b'             return results;\n',
    b'+        }\n',
    b'+        private String readerWitness(String nonce) {\n',
    b'+            if (!nonce.matches("[A-Za-z0-9_-]{1,64}")) {\n',
    b'+                throw new WebApplicationException("Invalid reader witness nonce", Response.Status.BAD_REQUEST);\n',
    b'+            }\n',
    b'+            if (selectedProjectNames == null || selectedProjectNames.isEmpty()\n',
    b'+                    || selectedProjectNames.size() != selectedProjectCount\n',
    b'+                    || engine.scoreDocs() == null || engine.getSearcher() == null\n',
    b'+                    || !(engine.getSearcher().getIndexReader() instanceof MultiReader multiReader)) {\n',
    b'+                throw new WebApplicationException("Search reader unavailable", Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+            }\n',
    b'+            List<IndexReaderContext> subreaders = multiReader.getContext().children();\n',
    b'+            if (subreaders == null || subreaders.size() != selectedProjectNames.size()) {\n',
    b'+                throw new WebApplicationException("Search reader project count mismatch",\n',
    b'+                        Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+            }\n',
    b'+            StringBuilder witness = new StringBuilder("v1:").append(nonce);\n',
    b'+            int i = 0;\n',
    b'+            try {\n',
    b'+                for (String name : selectedProjectNames) {\n',
    b'+                    IndexReader subreader = subreaders.get(i++).reader();\n',
    b'+                    if (!name.matches("[A-Za-z0-9._-]+") || !(subreader instanceof DirectoryReader directoryReader)) {\n',
    b'+                        throw new WebApplicationException("Search reader project type mismatch",\n',
    b'+                                Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+                    }\n',
    b'+                    IndexCommit commit = directoryReader.getIndexCommit();\n',
    b'+                    String segments = commit.getSegmentsFileName();\n',
    b'+                    if (!segments.matches("[A-Za-z0-9._-]+")) {\n',
    b'+                        throw new WebApplicationException("Invalid segments file name",\n',
    b'+                                Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+                    }\n',
    b'+                    MessageDigest digest = MessageDigest.getInstance("SHA-256");\n',
    b'+                    for (String file : new TreeSet<>(commit.getFileNames())) {\n',
    b'+                        digest.update(file.getBytes(StandardCharsets.UTF_8));\n',
    b'+                        digest.update((byte) 0);\n',
    b'+                    }\n',
    b"+                    witness.append(':').append(name).append(',').append(segments)\n",
    b"+                            .append(',').append(commit.getGeneration())\n",
    b"+                            .append(',').append(directoryReader.getVersion())\n",
    b"+                            .append(',').append(directoryReader.numDocs())\n",
    b"+                            .append(',').append(directoryReader.maxDoc())\n",
    b"+                            .append(',').append(HexFormat.of().formatHex(digest.digest()));\n",
    b'+                }\n',
    b'+            } catch (IOException | NoSuchAlgorithmException e) {\n',
    b'+                throw new WebApplicationException("Search reader witness failed",\n',
    b'+                        Response.Status.INTERNAL_SERVER_ERROR);\n',
    b'+            }\n',
    b'+            return witness.toString();\n',
    b'         }\n',
    b' \n',
    b'         private boolean isValid() {\n',
)
PATCH = b"".join(PATCH_LINES)
HUNK = re.compile(rb"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(?:[^\n]*)\n")


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _apply(original: bytes, patch: bytes) -> bytes:
    lines = patch.splitlines(keepends=True)
    if lines[:2] != [b"--- a/SearchController.java\n", b"+++ b/SearchController.java\n"]:
        raise ValueError("query reader patch targets another source")
    source = original.splitlines(keepends=True)
    output: list[bytes] = []
    cursor = 0
    index = 2
    hunks = 0
    while index < len(lines):
        header = HUNK.fullmatch(lines[index])
        if header is None:
            raise ValueError("query reader patch has a malformed hunk")
        old_start, old_count, new_start, new_count = (
            int(header[1]), int(header[2] or b"1"),
            int(header[3]), int(header[4] or b"1"),
        )
        if old_start <= cursor or old_start > len(source) + 1:
            raise ValueError("query reader patch hunk overlaps or escapes original")
        output.extend(source[cursor:old_start - 1])
        cursor = old_start - 1
        if len(output) + 1 != new_start:
            raise ValueError("query reader patch output offset differs")
        index += 1
        consumed = produced = 0
        while index < len(lines) and not lines[index].startswith(b"@@ "):
            line = lines[index]
            operation, data = line[:1], line[1:]
            if operation in (b" ", b"-"):
                if cursor >= len(source) or source[cursor] != data:
                    raise ValueError("query reader patch context differs from original")
                cursor += 1
                consumed += 1
            if operation in (b" ", b"+"):
                output.append(data)
                produced += 1
            if operation not in (b" ", b"-", b"+"):
                raise ValueError("query reader patch has an unsupported line")
            index += 1
        if (consumed, produced) != (old_count, new_count):
            raise ValueError("query reader patch hunk length differs")
        hunks += 1
    if hunks == 0:
        raise ValueError("query reader patch has no hunks")
    output.extend(source[cursor:])
    return b"".join(output)


def _validated_sources(original: bytes, patch: bytes | None = None) -> bytes:
    if patch is None:
        patch = PATCH
    if _sha(original) != ORIGINAL_SOURCE_SHA256:
        raise ValueError("OpenGrok upstream SearchController source SHA differs")
    if _sha(patch) != SOURCE_PATCH_SHA256:
        raise ValueError("OpenGrok query reader patch SHA differs")
    patched = _apply(original, patch)
    if _sha(patched) != PATCHED_SOURCE_SHA256:
        raise ValueError("OpenGrok patched SearchController source SHA differs")
    return patched


def materialize(original_path: Path, output_root: Path) -> Path:
    """Write the exact three pinned source artifacts into a fresh external root."""
    if not original_path.is_file() or original_path.stat().st_size > 1024 * 1024:
        raise ValueError("OpenGrok upstream source file is missing or unbounded")
    original = original_path.read_bytes()
    patched = _validated_sources(original)
    checkout = Path(__file__).resolve().parents[3]
    if not output_root.is_absolute() or output_root.resolve().is_relative_to(checkout):
        raise ValueError("query reader fixture output must be outside the checkout")
    if output_root.exists() or output_root.is_symlink() or not output_root.parent.is_dir():
        raise ValueError("query reader fixture output must be a fresh path")
    staging = Path(tempfile.mkdtemp(prefix=".qi-og-reader-fixture-", dir=output_root.parent))
    try:
        (staging / "original").mkdir()
        (staging / "patched").mkdir()
        (staging / "original/SearchController.java").write_bytes(original)
        (staging / "patched/SearchController.java").write_bytes(patched)
        (staging / "SearchController.patch").write_bytes(PATCH)
        staging.rename(output_root)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return output_root


def verify_class_outputs(classes_root: Path) -> dict[str, str]:
    """Accept exactly the four bytes observed from the pinned-image javac build."""
    if not classes_root.is_dir() or classes_root.is_symlink():
        raise ValueError("compiled class root differs")
    paths = list(classes_root.rglob("*"))
    if any(path.is_symlink() or (not path.is_file() and not path.is_dir()) for path in paths):
        raise ValueError("compiled class tree contains a link or special file")
    files = {path.relative_to(classes_root).as_posix(): path for path in paths if path.is_file()}
    if set(files) != set(CLASS_SHA256):
        raise ValueError("compiled class inventory differs")
    observed = {name: _sha(path.read_bytes()) for name, path in files.items()}
    if observed != CLASS_SHA256:
        raise ValueError("compiled class SHA differs from pinned build")
    return observed


def build_classes(fixture_root: Path, baseline_web_inf: Path) -> dict[str, str]:
    """Optionally run javac in the pinned local image, with no network or image pull."""
    checkout = Path(__file__).resolve().parents[3]
    if not fixture_root.is_absolute() or fixture_root.resolve().is_relative_to(checkout):
        raise ValueError("query reader fixture build root must be outside the checkout")
    original = (fixture_root / "original/SearchController.java").read_bytes()
    patched = _validated_sources(original)
    if (fixture_root / "patched/SearchController.java").read_bytes() != patched or (
        fixture_root / "SearchController.patch"
    ).read_bytes() != PATCH:
        raise ValueError("materialized fixture changed before compilation")
    web_inf = baseline_web_inf.resolve(strict=True)
    if not web_inf.is_dir() or not (web_inf / "classes").is_dir() or not (web_inf / "lib").is_dir():
        raise ValueError("baseline WEB-INF classpath differs")
    classes = fixture_root / "classes"
    if classes.exists() or classes.is_symlink():
        raise ValueError("compiled class output must be fresh")
    staging = Path(tempfile.mkdtemp(prefix=".qi-og-reader-classes-", dir=fixture_root))
    try:
        for path in (web_inf, fixture_root / "patched", staging):
            if "," in str(path):
                raise ValueError("Docker bind path contains a comma")
        command = [
            "docker", "run", "--rm", "--pull=never", "--network=none", "--read-only",
            "--cap-drop=ALL", "--security-opt=no-new-privileges",
            "--tmpfs", "/tmp:rw,nosuid,nodev,size=64m", "--user", "0:0",
            "--mount", f"type=bind,src={web_inf},dst=/baseline,readonly",
            "--mount", f"type=bind,src={fixture_root / 'patched'},dst=/candidate,readonly",
            "--mount", f"type=bind,src={staging},dst=/out",
            "--entrypoint", "/bin/sh", COMPILER_IMAGE_ID, "-ec",
            'javac -encoding UTF-8 -cp "/baseline/classes:/baseline/lib/*:/usr/local/tomcat/lib/servlet-api.jar" -d /out /candidate/SearchController.java',
        ]
        completed = subprocess.run(command, check=False, capture_output=True, timeout=180)
        if completed.returncode != 0:
            detail = completed.stderr.decode("utf-8", "replace")[-2000:]
            raise ValueError(f"pinned javac failed with exit {completed.returncode}: {detail}")
        observed = verify_class_outputs(staging)
        staging.rename(classes)
        return observed
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--original", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--build-web-inf", type=Path)
    args = parser.parse_args()
    if args.build_web_inf is None:
        materialize(args.original, args.output)
        return 0
    checkout = Path(__file__).resolve().parents[3]
    if (
        not args.output.is_absolute()
        or args.output.resolve().is_relative_to(checkout)
        or not args.output.parent.is_dir()
        or args.output.exists()
        or args.output.is_symlink()
    ):
        raise ValueError("query reader fixture output must be a fresh external path")
    holder = Path(tempfile.mkdtemp(prefix=".qi-og-reader-build-", dir=args.output.parent))
    try:
        fixture = materialize(args.original, holder / "fixture")
        build_classes(fixture, args.build_web_inf)
        if args.output.exists() or args.output.is_symlink():
            raise ValueError("query reader fixture output appeared during build")
        fixture.rename(args.output)
    finally:
        shutil.rmtree(holder)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
