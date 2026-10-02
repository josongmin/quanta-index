// Independent TypeScript/JavaScript declaration census over the TypeScript
// compiler parser (not Tree-sitter).
//
// Reads one file path per stdin line and writes one JSON object per line:
// {"path", "declarations": [[name, kind, utf8_byte_offset]]} or {"path", "error"}.
import fs from "node:fs";
import ts from "typescript";

const KINDS = new Map([
  [ts.SyntaxKind.FunctionDeclaration, "function"],
  [ts.SyntaxKind.ClassDeclaration, "class"],
  [ts.SyntaxKind.MethodDeclaration, "method"],
  [ts.SyntaxKind.GetAccessor, "get_accessor"],
  [ts.SyntaxKind.SetAccessor, "set_accessor"],
  [ts.SyntaxKind.MethodSignature, "method_signature"],
  [ts.SyntaxKind.InterfaceDeclaration, "interface"],
  [ts.SyntaxKind.TypeAliasDeclaration, "type_alias"],
  [ts.SyntaxKind.EnumDeclaration, "enum"],
]);

function scriptKind(path) {
  if (path.endsWith(".tsx")) return ts.ScriptKind.TSX;
  if (path.endsWith(".ts")) return ts.ScriptKind.TS;
  if (path.endsWith(".jsx")) return ts.ScriptKind.JSX;
  return ts.ScriptKind.JS;
}

function census(path, text) {
  const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, scriptKind(path));
  if (source.parseDiagnostics.length > 0) {
    const first = source.parseDiagnostics[0];
    return { error: "parse: " + ts.flattenDiagnosticMessageText(first.messageText, "\n") };
  }
  const found = [];
  const visit = (node) => {
    if (node.kind === ts.SyntaxKind.Constructor) {
      const keyword = node.getChildren(source).find((c) => c.kind === ts.SyntaxKind.ConstructorKeyword);
      if (keyword) found.push(["constructor", "constructor", keyword.getStart(source)]);
    } else if (KINDS.has(node.kind) && node.name) {
      if (ts.isIdentifier(node.name) || ts.isPrivateIdentifier(node.name)) {
        found.push([node.name.text, KINDS.get(node.kind), node.name.getStart(source)]);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  // UTF-16 code-unit positions -> UTF-8 byte offsets in one ordered pass.
  const order = found.map((row, index) => [row[2], index]).sort((a, b) => a[0] - b[0]);
  let unit = 0;
  let bytes = 0;
  for (const [position, index] of order) {
    bytes += Buffer.byteLength(text.slice(unit, position), "utf8");
    unit = position;
    found[index][2] = bytes;
  }
  return { declarations: found };
}

const paths = fs.readFileSync(0, "utf8").split("\n").filter((line) => line.length > 0);
for (const path of paths) {
  let result;
  try {
    result = census(path, fs.readFileSync(path, "utf8"));
  } catch (error) {
    result = { error: "read: " + String(error) };
  }
  process.stdout.write(JSON.stringify({ path, ...result }) + "\n");
}
