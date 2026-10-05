import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import org.apache.lucene.document.Document;
import org.apache.lucene.index.DirectoryReader;
import org.apache.lucene.index.FieldInfo;
import org.apache.lucene.index.IndexOptions;
import org.apache.lucene.index.IndexableField;
import org.apache.lucene.index.LeafReader;
import org.apache.lucene.index.LeafReaderContext;
import org.apache.lucene.index.PostingsEnum;
import org.apache.lucene.index.SegmentReader;
import org.apache.lucene.index.Terms;
import org.apache.lucene.index.TermsEnum;
import org.apache.lucene.search.DocIdSetIterator;
import org.apache.lucene.store.Directory;
import org.apache.lucene.store.FSDirectory;
import org.apache.lucene.util.Bits;
import org.apache.lucene.util.BytesRef;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;

/** Bounded native live-doc/postings observation; Python validates document roles. */
public final class FullLiveDocuments {
  // Map.of iteration order changes between JVMs; query brackets need identical bytes.
  private static final ObjectMapper JSON = new ObjectMapper()
      .enable(SerializationFeature.ORDER_MAP_ENTRIES_BY_KEYS);
  private static final long OUTPUT_CAP = 256L * 1024 * 1024;
  private static final int ROW_CAP = 4 * 1024 * 1024;
  private static long outputBytes = 0;
  private static final long DEADLINE = System.nanoTime() + 3_600_000_000_000L;

  private static void bounded() {
    if (System.nanoTime() - DEADLINE >= 0) {
      throw new IllegalStateException("full live-document observation exceeded native deadline");
    }
  }

  private static MessageDigest sha() throws Exception {
    return MessageDigest.getInstance("SHA-256");
  }

  private static String hex(byte[] bytes) {
    return java.util.HexFormat.of().formatHex(bytes);
  }

  private static void emit(Map<String, Object> row) throws Exception {
    byte[] line = JSON.writeValueAsString(row).getBytes(StandardCharsets.UTF_8);
    if (line.length > ROW_CAP || outputBytes > OUTPUT_CAP - line.length - 1) {
      throw new IllegalStateException("full live-document output exceeds byte bound");
    }
    System.out.write(line);
    System.out.write('\n');
    outputBytes += line.length + 1L;
  }

  private static Map<String, Object> value(IndexableField field) {
    Map<String, Object> result = new LinkedHashMap<>();
    result.put("name", field.name());
    result.put("stored", field.fieldType().stored());
    result.put("indexOptions", field.fieldType().indexOptions().toString());
    result.put("docValuesType", field.fieldType().docValuesType().toString());
    BytesRef binary = field.binaryValue();
    Number number = field.numericValue();
    String string = field.stringValue();
    if (binary != null) {
      result.put("valueKind", "binary");
      result.put("valueBase64", Base64.getEncoder().encodeToString(
          java.util.Arrays.copyOfRange(binary.bytes, binary.offset, binary.offset + binary.length)));
    } else if (number != null) {
      result.put("valueKind", "number");
      result.put("numberClass", number.getClass().getName());
      result.put("valueDecimal", number.toString());
    } else if (string != null) {
      result.put("valueKind", "string");
      result.put("value", string);
    } else {
      result.put("valueKind", "missing");
    }
    return result;
  }

  private static Map<String, Object> named(Document doc, String name) {
    List<Map<String, Object>> values = new ArrayList<>();
    for (IndexableField field : doc.getFields(name)) {
      values.add(value(field));
    }
    return Map.of("present", !values.isEmpty(), "values", values);
  }

  private static final class TermSummary {
    private final MessageDigest digest;
    private long distinct = 0;
    private long occurrences = 0;

    private TermSummary() throws Exception { digest = sha(); }

    private void add(BytesRef term, int frequency) {
      if (frequency <= 0) throw new IllegalStateException("nonpositive Lucene term frequency");
      distinct = Math.addExact(distinct, 1);
      occurrences = Math.addExact(occurrences, frequency);
      digest.update(ByteBuffer.allocate(4).putInt(term.length).array());
      digest.update(term.bytes, term.offset, term.length);
      digest.update(ByteBuffer.allocate(4).putInt(frequency).array());
    }

    private Map<String, Object> result(String field, boolean frequenciesAvailable) {
      return Map.of("field", field, "distinctTerms", distinct,
          "occurrences", occurrences, "termFrequencySha256", hex(digest.digest()),
          "frequenciesAvailable", frequenciesAvailable);
    }
  }

  private static Map<String, Object> segmentField(FieldInfo info) {
    return Map.of("name", info.name, "indexOptions", info.getIndexOptions().toString(),
        "docValuesType", info.getDocValuesType().toString(),
        "hasVectors", info.hasVectors(), "hasNorms", info.hasNorms());
  }

  public static void main(String[] args) throws Exception {
    if (args.length != 2 || !Path.of(args[0]).isAbsolute()) {
      throw new IllegalArgumentException("absolute index root and comma-separated repositories required");
    }
    List<String> repos = new ArrayList<>(List.of(args[1].split(",", -1)));
    if (repos.isEmpty() || repos.stream().anyMatch(String::isBlank)
        || repos.size() != repos.stream().distinct().count()) {
      throw new IllegalArgumentException("repository list is empty or repeated");
    }
    List<String> sorted = new ArrayList<>(repos);
    Collections.sort(sorted);
    if (!sorted.equals(repos)) throw new IllegalArgumentException("repository list must be sorted");
    if (repos.stream().anyMatch(repo -> !repo.matches("[A-Za-z0-9][A-Za-z0-9_.-]*"))) {
      throw new IllegalArgumentException("repository is not a single path component");
    }
    try (var paths = Files.list(Path.of(args[0]))) {
      List<String> actual = paths.map(path -> {
        if (Files.isSymbolicLink(path) || !Files.isDirectory(path)) {
          throw new IllegalStateException("index root contains an unknown project or special file");
        }
        return path.getFileName().toString();
      }).sorted().toList();
      if (!actual.equals(repos)) throw new IllegalStateException("native project universe differs");
    }
    long allLive = 0;
    for (String repo : repos) {
      bounded();
      long liveCount = 0;
      long pathPresent = 0;
      long pathMissing = 0;
      long pathFieldMissing = 0;
      try (Directory dir = FSDirectory.open(Path.of(args[0], repo));
           DirectoryReader index = DirectoryReader.open(dir)) {
        for (LeafReaderContext leaf : index.leaves()) {
          bounded();
          LeafReader reader = leaf.reader();
          if (!(reader instanceof SegmentReader segment)) {
            throw new IllegalStateException("Lucene leaf is not a native SegmentReader");
          }
          Bits live = reader.getLiveDocs();
          List<FieldInfo> infos = new ArrayList<>();
          for (FieldInfo info : reader.getFieldInfos()) infos.add(info);
          infos.sort(java.util.Comparator.comparing(info -> info.name));
          List<Map<String, Object>> fieldInfo = new ArrayList<>();
          Map<String, TermSummary[]> termsByField = new TreeMap<>();
          Map<String, Boolean> frequencyModes = new TreeMap<>();
          for (FieldInfo info : infos) {
            fieldInfo.add(segmentField(info));
            if (info.getIndexOptions() == IndexOptions.NONE) continue;
            Terms terms = reader.terms(info.name);
            if (terms == null) continue;
            boolean hasFreq = info.getIndexOptions().compareTo(IndexOptions.DOCS_AND_FREQS) >= 0;
            TermSummary[] perDoc = new TermSummary[reader.maxDoc()];
            TermsEnum iterator = terms.iterator();
            PostingsEnum postings = null;
            BytesRef term;
            while ((term = iterator.next()) != null) {
              bounded();
              postings = iterator.postings(postings, hasFreq ? PostingsEnum.FREQS : PostingsEnum.NONE);
              if (postings == null) throw new IllegalStateException("indexed term has no postings");
              for (int doc = postings.nextDoc(); doc != DocIdSetIterator.NO_MORE_DOCS;
                   doc = postings.nextDoc()) {
                if (live != null && !live.get(doc)) continue;
                if (perDoc[doc] == null) perDoc[doc] = new TermSummary();
                perDoc[doc].add(term, hasFreq ? postings.freq() : 1);
              }
            }
            termsByField.put(info.name, perDoc);
            frequencyModes.put(info.name, hasFreq);
          }
          emit(Map.of("kind", "segment", "repository", repo, "segmentName", segment.getSegmentName(),
              "leafOrdinal", leaf.ord, "docBase", leaf.docBase,
              "maxDoc", reader.maxDoc(), "numDocs", reader.numDocs(), "fieldInfo", fieldInfo));
          long segmentLive = 0;
          for (int doc = 0; doc < reader.maxDoc(); doc++) {
            if (live != null && !live.get(doc)) continue;
            bounded();
            segmentLive++;
            Document stored = reader.storedFields().document(doc);
            List<Map<String, Object>> storedFields = new ArrayList<>();
            for (IndexableField field : stored.getFields()) storedFields.add(value(field));
            Map<String, Object> selected = new TreeMap<>();
            for (String name : List.of("path", "u", "type", "t", "project", "date")) {
              selected.put(name, named(stored, name));
            }
            List<Map<String, Object>> indexedFields = new ArrayList<>();
            for (Map.Entry<String, TermSummary[]> entry : termsByField.entrySet()) {
              TermSummary summary = entry.getValue()[doc];
              if (summary != null) indexedFields.add(summary.result(entry.getKey(), frequencyModes.get(entry.getKey())));
            }
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("kind", "document"); row.put("repository", repo);
            row.put("segmentName", segment.getSegmentName()); row.put("leafOrdinal", leaf.ord);
            row.put("docLocal", doc); row.put("docGlobal", leaf.docBase + doc);
            row.put("storedFields", storedFields); row.put("selectedStoredFields", selected);
            row.put("pathStringPresent", stored.get("path") != null);
            row.put("indexedFields", indexedFields);
            emit(row);
            if (stored.get("path") == null) pathMissing++; else pathPresent++;
            if (stored.getFields("path").length == 0) pathFieldMissing++;
          }
          if (segmentLive != reader.numDocs()) throw new IllegalStateException("segment live-doc count differs");
          liveCount += segmentLive;
        }
        if (liveCount != index.numDocs()) throw new IllegalStateException("repository live-doc count differs");
      }
      allLive += liveCount;
      emit(Map.of("kind", "repository_summary", "repository", repo,
          "liveDocs", liveCount, "pathPresent", pathPresent, "pathMissing", pathMissing,
          "pathFieldMissing", pathFieldMissing));
    }
    emit(Map.of("kind", "terminal", "repositories", repos, "liveDocs", allLive));
  }
}
