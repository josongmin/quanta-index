// Candidate-only, same-byte-width projection for released JS/TS grammars.
// The TypeScript compiler approves the original and independently compares
// named declaration ownership/ranges before and after each projection.
const crypto = require('node:crypto');
const fs = require('node:fs');

const schema = 'quanta-ts-grammar-projection-v1';
const maxSourceBytes = 1 << 20;
const maxEdits = 1 << 16;
const declarationKinds = new Set([
  'FunctionDeclaration', 'FunctionExpression', 'ClassDeclaration', 'ClassExpression',
  'MethodDeclaration', 'MethodSignature', 'GetAccessor', 'SetAccessor',
  'InterfaceDeclaration', 'TypeAliasDeclaration', 'EnumDeclaration', 'ModuleDeclaration',
]);

function byteOffset(text, index) {
  return Buffer.byteLength(text.slice(0, index), 'utf8');
}

function inventory(ts, file, text) {
  const rows = [];
  const visit = (node, owners) => {
    const kind = ts.SyntaxKind[node.kind];
    const name = node.name;
    const named = declarationKinds.has(kind) && name != null;
    let nextOwners = owners;
    if (named) {
      const nameText = text.slice(name.getStart(file), name.end);
      const item = [kind, nameText, owners.join('.'),
        byteOffset(text, name.getStart(file)), byteOffset(text, name.end),
        byteOffset(text, node.getStart(file)), byteOffset(text, node.end)];
      rows.push(item);
      nextOwners = [...owners, nameText];
    }
    ts.forEachChild(node, child => visit(child, nextOwners));
  };
  visit(file, []);
  rows.sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
  return JSON.stringify(rows);
}

function blankSpan(raw, start, end, leading) {
  const original = raw.subarray(start, end);
  const replacement = Buffer.from(original.map(byte => byte === 10 || byte === 13 ? byte : 32));
  if (leading.length > replacement.length || leading.includes('\n') || leading.includes('\r')) {
    throw Error('replacement exceeds span');
  }
  replacement.write(leading, 0, 'ascii');
  return replacement.toString('ascii');
}

function makeEdits(ts, file, text, raw, kind) {
  const edits = [];
  const add = (startUnit, endUnit, leading) => {
    const start = byteOffset(text, startUnit);
    const end = byteOffset(text, endUnit);
    edits.push({start, end, replacement: blankSpan(raw, start, end, leading)});
  };
  const visit = node => {
    if ((kind === 'javascript' || kind === 'jsx') && ts.isExportSpecifier(node)
        && node.propertyName && node.name) {
      const start = node.name.getStart(file);
      const end = node.name.end;
      const alias = text.slice(start, end);
      if (new Set(['const', 'var', 'true', 'false', 'for', 'function', 'return',
        'if', 'this', 'null', 'debugger', 'new']).has(alias)) {
        add(start, end, 'a'.repeat(Buffer.byteLength(alias)));
      }
    }
    if ((kind === 'typescript' || kind === 'tsx') && ts.isExportDeclaration(node)
        && node.isTypeOnly && (!node.exportClause || ts.isNamespaceExport(node.exportClause))) {
      const typeToken = node.getChildren(file).find(child => child.kind === ts.SyntaxKind.TypeKeyword);
      if (!typeToken) throw Error('type-only star export lacks token');
      add(typeToken.getStart(file), typeToken.end, '');
    }
    if ((kind === 'typescript' || kind === 'tsx') && ts.isCallExpression(node)
        && node.typeArguments) {
      for (const argument of node.typeArguments) {
        const visitType = child => {
          if (ts.isImportTypeNode(child)) {
            add(child.getStart(file), child.end, 'any');
            return;
          }
          ts.forEachChild(child, visitType);
        };
        visitType(argument);
      }
    }
    if ((kind === 'typescript' || kind === 'tsx') && ts.isTypeLiteralNode(node)) {
      const members = node.members;
      for (let i = 1; i < members.length; i++) {
        const previous = members[i - 1];
        const next = members[i];
        if (!ts.isCallSignatureDeclaration(previous) || !ts.isCallSignatureDeclaration(next)
            || !previous.type) continue;
        const gap = text.slice(previous.end, next.getStart(file));
        if (!/[\r\n]/.test(gap) || /[;,]/.test(gap)) continue;
        const typeStart = previous.type.getStart(file);
        const colon = text.lastIndexOf(':', typeStart);
        if (colon < previous.parameters.end || !/^\s*$/.test(text.slice(colon + 1, typeStart))) {
          throw Error('call signature return annotation boundary invalid');
        }
        add(colon, previous.type.end, ';');
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  edits.sort((left, right) => left.start - right.start);
  if (edits.length > maxEdits) return null;
  let last = 0;
  for (const edit of edits) {
    if (edit.start < last || edit.end > raw.length || edit.start >= edit.end
        || Buffer.byteLength(edit.replacement) !== edit.end - edit.start) {
      throw Error('overlapping or unequal edits');
    }
    last = edit.end;
  }
  return edits;
}

function project(ts, path, raw) {
  const result = {schema, status: 'ok', typescript_version: ts.version,
    node_version: process.version, source_sha256: crypto.createHash('sha256').update(raw).digest('hex'),
    edits: []};
  if (raw.length > maxSourceBytes) return {...result, status: 'resource_limit'};
  let text;
  try { text = new TextDecoder('utf-8', {fatal: true}).decode(raw); }
  catch { return {...result, status: 'parse_failure'}; }
  const suffix = path.slice(path.lastIndexOf('.'));
  const kind = ({'.js': 'javascript', '.mjs': 'javascript', '.cjs': 'javascript',
    '.jsx': 'jsx', '.ts': 'typescript', '.mts': 'typescript', '.cts': 'typescript',
    '.tsx': 'tsx'})[suffix];
  if (!kind) return {...result, status: 'producer_defect'};
  const scriptKind = kind === 'tsx' ? ts.ScriptKind.TSX
    : kind === 'typescript' ? ts.ScriptKind.TS
    : kind === 'jsx' ? ts.ScriptKind.JSX : ts.ScriptKind.JS;
  const file = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, scriptKind);
  if (file.parseDiagnostics.length) return {...result, status: 'parse_failure'};
  let edits;
  try { edits = makeEdits(ts, file, text, raw, kind); }
  catch { return {...result, status: 'producer_defect'}; }
  if (edits === null) return {...result, status: 'resource_limit'};
  const projected = Buffer.from(raw);
  for (const edit of edits) projected.write(edit.replacement, edit.start, 'ascii');
  const projectedText = new TextDecoder('utf-8', {fatal: true}).decode(projected);
  const projectedFile = ts.createSourceFile(path, projectedText, ts.ScriptTarget.Latest, true, scriptKind);
  if (projectedFile.parseDiagnostics.length
      || inventory(ts, file, text) !== inventory(ts, projectedFile, projectedText)) {
    return {...result, status: 'producer_defect'};
  }
  return {...result, edits};
}

function readSourceLimited() {
  const buffer = Buffer.alloc(maxSourceBytes + 1);
  let length = 0;
  while (length < buffer.length) {
    const read = fs.readSync(0, buffer, length, buffer.length - length);
    if (read === 0) break;
    length += read;
  }
  return buffer.subarray(0, length);
}

if (require.main === module) {
  if (process.argv.length !== 4) process.exit(2);
  const [path, tsModule] = process.argv.slice(2);
  let ts;
  try { ts = require(tsModule); }
  catch { process.exit(2); }
  if (ts.version !== '5.9.3') process.exit(2);
  const raw = readSourceLimited();
  process.stdout.write(JSON.stringify(project(ts, path, raw)) + '\n');
}

module.exports = {project};
