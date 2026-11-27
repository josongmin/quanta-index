// Candidate-only, same-byte-width projection for released JS/TS grammars.
// The TypeScript compiler approves the original and independently compares
// named declaration ownership/ranges before and after each projection.
const crypto = require('node:crypto');
const fs = require('node:fs');
const pathModule = require('node:path');
const {Worker, isMainThread, parentPort, workerData} = require('node:worker_threads');

const schema = 'quanta-ts-grammar-projection-v1';
const maxSourceBytes = 1 << 20;
const maxBatchInputBytes = 16 << 20;
const maxBatchOutputBytes = 8 << 20;
const maxBatchFiles = 32;
const maxEdits = 1 << 16;
const declarationKinds = new Set([
  'FunctionDeclaration', 'FunctionExpression', 'ClassDeclaration', 'ClassExpression',
  'MethodDeclaration', 'MethodSignature', 'GetAccessor', 'SetAccessor',
  'InterfaceDeclaration', 'TypeAliasDeclaration', 'EnumDeclaration', 'ModuleDeclaration',
]);
const reservedExportAliases = new Set([
  'const', 'var', 'true', 'false', 'for', 'function', 'return',
  'if', 'this', 'null', 'debugger', 'new',
]);

// TypeScript positions are UTF-16 code units; all producer spans are UTF-8 bytes.
function byteOffsets(text) {
  if (!/[^\x00-\x7f]/u.test(text)) return index => index;
  const offsets = new Int32Array(text.length + 1);
  let bytes = 0;
  for (let index = 0; index < text.length; index++) {
    offsets[index] = bytes;
    const unit = text.charCodeAt(index);
    if (unit < 0x80) bytes += 1;
    else if (unit < 0x800) bytes += 2;
    else if (unit >= 0xd800 && unit <= 0xdbff) {
      offsets[++index] = -1;
      bytes += 4;
    } else bytes += 3;
  }
  offsets[text.length] = bytes;
  return index => {
    const offset = offsets[index];
    if (offset === undefined || offset < 0) throw Error('invalid UTF-16 boundary');
    return offset;
  };
}

function inventory(ts, file, text, byteOffset) {
  const rows = [];
  const visit = (node, owners) => {
    const kind = ts.SyntaxKind[node.kind];
    const name = node.name;
    const named = declarationKinds.has(kind) && name != null;
    let nextOwners = owners;
    if (named) {
      const nameStart = name.getStart(file);
      const nameText = text.slice(nameStart, name.end);
      rows.push(JSON.stringify([kind, nameText, owners,
        byteOffset(nameStart), byteOffset(name.end),
        byteOffset(node.getStart(file)), byteOffset(node.end)]));
      nextOwners = [...owners, nameText];
    }
    ts.forEachChild(node, child => visit(child, nextOwners));
  };
  visit(file, []);
  return rows.sort().join('\n');
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

function makeEdits(ts, file, text, raw, kind, byteOffset) {
  const edits = [];
  const add = (startUnit, endUnit, leading) => {
    const start = byteOffset(startUnit);
    const end = byteOffset(endUnit);
    edits.push({start, end, replacement: blankSpan(raw, start, end, leading)});
  };
  const visit = node => {
    if ((kind === 'javascript' || kind === 'jsx') && ts.isExportSpecifier(node)
        && node.propertyName && node.name) {
      const start = node.name.getStart(file);
      const end = node.name.end;
      const alias = text.slice(start, end);
      if (reservedExportAliases.has(alias)) {
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
  const decoder = new TextDecoder('utf-8', {fatal: true, ignoreBOM: true});
  try { text = decoder.decode(raw); }
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
  try {
    const byteOffset = byteOffsets(text);
    const edits = makeEdits(ts, file, text, raw, kind, byteOffset);
    if (edits === null) return {...result, status: 'resource_limit'};
    if (edits.length === 0) return result;
    const projected = Buffer.from(raw);
    for (const edit of edits) projected.write(edit.replacement, edit.start, 'ascii');
    const projectedText = decoder.decode(projected);
    const projectedFile = ts.createSourceFile(path, projectedText, ts.ScriptTarget.Latest, true, scriptKind);
    if (projectedFile.parseDiagnostics.length
        || inventory(ts, file, text, byteOffset)
        !== inventory(ts, projectedFile, projectedText, byteOffsets(projectedText))) {
      return {...result, status: 'producer_defect'};
    }
    return {...result, edits};
  } catch {
    return {...result, status: 'producer_defect'};
  }
}

function readSourceLimited(maxBytes = maxSourceBytes) {
  const buffer = Buffer.alloc(maxBytes + 1);
  let length = 0;
  while (length < buffer.length) {
    const read = fs.readSync(0, buffer, length, buffer.length - length);
    if (read === 0) break;
    length += read;
  }
  return buffer.subarray(0, length);
}

function readBatch() {
  const input = readSourceLimited(maxBatchInputBytes);
  if (input.length > maxBatchInputBytes) throw Error('batch input exceeds limit');
  const json = new TextDecoder('utf-8', {fatal: true, ignoreBOM: true}).decode(input);
  const files = JSON.parse(json);
  if (!Array.isArray(files) || files.length < 1 || files.length > maxBatchFiles) {
    throw Error('invalid batch size');
  }
  for (const file of files) {
    if (file === null || typeof file !== 'object' || Array.isArray(file)
        || Object.keys(file).sort().join(',') !== 'path,source'
        || typeof file.path !== 'string' || !file.path || file.path.includes('\0')
        || typeof file.source !== 'string'
        || Buffer.byteLength(file.source, 'utf8') > maxSourceBytes
        || Buffer.from(file.source, 'utf8').toString('utf8') !== file.source) {
      throw Error('invalid batch file');
    }
  }
  return files;
}

function timeoutResponse(raw) {
  return {schema, status: 'timed_out', typescript_version: '5.9.3',
    node_version: process.version, source_sha256: crypto.createHash('sha256').update(raw).digest('hex'),
    edits: []};
}

function runBatchFile(current, tsModule, file, requestId, timeoutMs) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let worker = current;
    const raw = Buffer.from(file.source, 'utf8');
    const started = process.hrtime.bigint();
    const elapsedMs = () => Number((process.hrtime.bigint() - started + 999999n) / 1000000n);
    const cleanup = () => {
      settled = true;
      clearTimeout(timer);
      worker.thread.off('message', onMessage);
      worker.thread.off('error', onError);
      worker.thread.off('exit', onExit);
    };
    const onError = error => {
      if (!settled) {
        cleanup();
        worker.thread.terminate().catch(() => {});
        reject(error);
      }
    };
    const onExit = code => {
      if (!settled) { cleanup(); reject(Error(`batch worker exited ${code}`)); }
    };
    const onMessage = message => {
      if (settled) return;
      const observedElapsed = elapsedMs();
      if (observedElapsed > timeoutMs) { onTimeout(); return; }
      if (message?.type === 'ready' && !worker.ready) {
        worker.ready = true;
        worker.thread.postMessage({requestId, path: file.path, source: file.source});
      } else if (message?.type === 'result' && message.requestId === requestId && worker.ready) {
        cleanup();
        resolve({worker, elapsed_ms: observedElapsed, response: message.response});
      } else {
        cleanup();
        worker.thread.terminate().catch(() => {});
        reject(Error('invalid batch worker response'));
      }
    };
    // The timer starts before worker startup, so compiler loading is included in the deadline.
    const onTimeout = () => {
      if (settled) return;
      cleanup();
      worker.thread.terminate().then(
        () => resolve({worker: null, elapsed_ms: timeoutMs, response: timeoutResponse(raw)}), reject);
    };
    const timer = setTimeout(onTimeout, timeoutMs);
    try {
      if (!worker) {
        worker = {thread: new Worker(__filename, {
          workerData: {quantaBatchWorker: true, tsModule}, execArgv: [],
        }), ready: false};
      }
      worker.thread.on('message', onMessage);
      worker.thread.on('error', onError);
      worker.thread.on('exit', onExit);
      if (worker.ready) {
        worker.thread.postMessage({requestId, path: file.path, source: file.source});
      }
    } catch (error) {
      clearTimeout(timer);
      if (worker?.thread) {
        worker.thread.off('message', onMessage);
        worker.thread.off('error', onError);
        worker.thread.off('exit', onExit);
        worker.thread.terminate();
      }
      reject(error);
    }
  });
}

async function projectBatch(tsModule, timeoutMs) {
  const files = readBatch();
  const encodedRows = [];
  let outputBytes = 3; // '[', ']', and the trailing newline.
  let worker = null;
  try {
    for (let index = 0; index < files.length; index++) {
      const result = await runBatchFile(worker, tsModule, files[index], index, timeoutMs);
      worker = result.worker;
      const encodedRow = JSON.stringify({path: files[index].path,
        elapsed_ms: result.elapsed_ms, response: result.response});
      outputBytes += Buffer.byteLength(encodedRow, 'utf8') + (index > 0 ? 1 : 0);
      if (outputBytes > maxBatchOutputBytes) {
        throw Error('batch output exceeds limit');
      }
      encodedRows.push(encodedRow);
    }
    return `[${encodedRows.join(',')}]\n`;
  } finally {
    if (worker) await worker.thread.terminate();
  }
}

if (!isMainThread && workerData?.quantaBatchWorker === true) {
  try {
    const ts = require(workerData.tsModule);
    if (ts.version !== '5.9.3') throw Error('wrong TypeScript version');
    parentPort.on('message', ({requestId, path, source}) => {
      try {
        parentPort.postMessage({type: 'result', requestId,
          response: project(ts, path, Buffer.from(source, 'utf8'))});
      } catch {
        parentPort.postMessage({type: 'fatal'});
      }
    });
    parentPort.postMessage({type: 'ready'});
  } catch {
    parentPort.postMessage({type: 'fatal'});
  }
} else if (require.main === module) {
  if (process.argv[2] === '--batch') {
    const [, , , tsModule, timeoutArg] = process.argv;
    if (process.argv.length !== 5 || !pathModule.isAbsolute(tsModule || '')
        || !/^[1-9][0-9]*$/.test(timeoutArg || '')
        || !Number.isSafeInteger(Number(timeoutArg)) || Number(timeoutArg) > 600000) {
      process.exit(2);
    }
    projectBatch(tsModule, Number(timeoutArg)).then(
      output => process.stdout.write(output),
      error => { process.stderr.write(`batch projection failed: ${error.message}\n`); process.exitCode = 2; });
  } else {
    if (process.argv.length !== 4) process.exit(2);
    const [path, tsModule] = process.argv.slice(2);
    let ts;
    try { ts = require(tsModule); }
    catch { process.exit(2); }
    if (ts.version !== '5.9.3') process.exit(2);
    const raw = readSourceLimited();
    process.stdout.write(JSON.stringify(project(ts, path, raw)) + '\n');
  }
}

module.exports = {project};
