"""Replay a bounded Zoekt path/content audit against a query capture's index.

This proves stored document bytes and the complete selected repository path
set. It does not prove every posting, relevance labels, or fair performance.
"""

from __future__ import annotations

import base64
import binascii
import json
import math
import os
import re
import secrets
import selectors
import signal
import subprocess
import sys
import time
from pathlib import Path

SOURCE_ROOT = Path(__file__).resolve().parents[3]
if str(SOURCE_ROOT) not in sys.path:
    sys.path.insert(0, str(SOURCE_ROOT))

from tools.benchmark.evidence import (  # noqa: E402
    RawFile,
    _read_control_file,
    canonical_json,
    parse_json,
)
from tools.benchmark.retrieval import sourcegraph  # noqa: E402

SCOPE = "indexed_path_inventory_and_native_stored_document_bytes"
MAX_STREAM_BYTES = 16 * 1024 * 1024
MAX_NATIVE_BODY_BYTES = 16 * 1024 * 1024
MAX_TOTAL_NATIVE_BYTES = 512 * 1024 * 1024
MAX_NATIVE_CAPTURE_SECONDS = 3600
MAX_NATIVE_ROW_SECONDS = 120
MAX_NATIVE_STDERR_BYTES = 64 * 1024
MAX_NATIVE_JOBS_BYTES = 16 * 1024 * 1024
MAX_TRANSLATOR_BYTES = 256 * 1024 * 1024

_NATIVE_WORKER = """import base64, hashlib, json, os, signal, sys, time, urllib.parse, urllib.request
limit = 16 * 1024 * 1024
port = int(sys.argv[1])
invocation = sys.argv[2]
cancel_path = "/tmp/qi-sg-owned-reader-" + invocation + ".cancel"
if os.path.exists(cancel_path):
    raise SystemExit(7)
stat = open("/proc/self/stat").read().rpartition(") ")[2].split()
print(json.dumps({"kind": "native_worker_start", "invocation_id": invocation, "pid": __import__("os").getpid(), "start_ticks": int(stat[19])}, sort_keys=True), file=sys.stderr, flush=True)
jobs = json.loads(sys.stdin.readline())
if os.path.exists(cancel_path):
    raise SystemExit(7)
def deadline(_signum, _frame):
    raise TimeoutError("native document read exceeded 120 seconds")
signal.signal(signal.SIGALRM, deadline)
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, url):
        return None
opener = urllib.request.build_opener(NoRedirect)
for job in jobs:
    if os.path.exists(cancel_path):
        raise SystemExit(7)
    started = time.monotonic()
    url = "http://127.0.0.1:" + str(port) + "/print?" + urllib.parse.urlencode({"r": "benchmark/" + job["repository"], "f": job["path"], "format": "raw"})
    try:
        signal.setitimer(signal.ITIMER_REAL, 120)
        with opener.open(url, timeout=60) as response:
            status = response.status
            body = response.read(limit + 1)
        if len(body) > limit:
            raise ValueError("native stored body exceeds 16 MiB")
        actual = hashlib.sha256(body).hexdigest()
        row = {**job, "http_status": status, "actual_sha256": actual, "bytes": len(body), "matches": status == 200 and actual == job["file_sha256"], "seconds": time.monotonic() - started, "body_base64": base64.b64encode(body).decode("ascii")}
    except Exception as error:
        row = {**job, "matches": False, "error_type": type(error).__name__, "error": str(error), "seconds": time.monotonic() - started}
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    print(json.dumps(row, sort_keys=True, separators=(",", ":")), flush=True)
"""

_OWNED_WEB_SUPERVISOR = """import hashlib,json,os,select,stat,subprocess,sys,time
token, binary, index, port = sys.argv[1:5]
tombstone = "/tmp/qi-sg-owned-web-" + token + ".cancel"
if os.path.exists(tombstone):
    raise SystemExit(7)
source = os.stat(binary)
if os.path.islink(binary) or not stat.S_ISREG(source.st_mode) or not source.st_mode & stat.S_IXUSR:
    raise SystemExit(8)
h = hashlib.sha256()
with open(binary, "rb") as stream:
    for block in iter(lambda: stream.read(1048576), b""):
        h.update(block)
def stable(value):
    return (value.st_dev,value.st_ino,value.st_size,value.st_mtime_ns,value.st_ctime_ns,value.st_mode)
if stable(os.stat(binary)) != stable(source) or os.path.exists(tombstone):
    raise SystemExit(9)
argv = [binary, "-index", index, "-listen", "127.0.0.1:" + port, "-html=true"]
env = {**os.environ, "QI_SG_NATIVE_OWNER": token}
child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, env=env)
def ticks(pid):
    return int(open("/proc/" + str(pid) + "/stat").read().rpartition(") ")[2].split()[19])
try:
    if os.path.exists(tombstone):
        raise SystemExit(7)
    print(json.dumps({"kind":"owned_zoekt_webserver","invocation_id":token,
        "supervisor_pid":os.getpid(),"supervisor_start_ticks":ticks(os.getpid()),
        "child_pid":child.pid,"child_start_ticks":ticks(child.pid),
        "guest_path":binary,"guest_sha256":h.hexdigest(),
        "guest_dev_major":os.major(source.st_dev),"guest_dev_minor":os.minor(source.st_dev),
        "guest_inode":source.st_ino,"guest_size":source.st_size,
        "argv":argv},sort_keys=True),flush=True)
    # 3600 s reader plus bounded start, two binary copies and postflight RPCs.
    end = time.monotonic() + 5400
    while child.poll() is None and time.monotonic() < end and not os.path.exists(tombstone):
        if select.select([sys.stdin.buffer], [], [], 0.2)[0] and not sys.stdin.buffer.read(1):
            break
finally:
    if child.poll() is None:
        child.kill()
    child.wait()
"""

_OWNED_WEB_INSPECT = """import hashlib,json,os,stat,sys
pid, ticks, token, binary, index, port = sys.argv[1:7]
pid = int(pid); ticks = int(ticks)
raw_stat = open("/proc/" + str(pid) + "/stat").read().rpartition(") ")[2].split()
if int(raw_stat[19]) != ticks or raw_stat[0] == "Z":
    raise SystemExit(2)
argv = open("/proc/" + str(pid) + "/cmdline", "rb").read().split(b"\\0")[:-1]
expected = [binary, "-index", index, "-listen", "127.0.0.1:" + port, "-html=true"]
if argv != [part.encode() for part in expected]:
    raise SystemExit(3)
environ = open("/proc/" + str(pid) + "/environ", "rb").read().split(b"\\0")
if ("QI_SG_NATIVE_OWNER=" + token).encode() not in environ:
    raise SystemExit(4)
source = os.stat(binary)
if os.path.islink(binary) or not stat.S_ISREG(source.st_mode):
    raise SystemExit(5)
guest = []
for line in open("/proc/" + str(pid) + "/maps"):
    fields = line.split(maxsplit=5)
    if len(fields) != 6 or fields[5].strip() != binary:
        continue
    major, minor = (int(part, 16) for part in fields[3].split(":"))
    if (major, minor, int(fields[4])) != (os.major(source.st_dev), os.minor(source.st_dev), source.st_ino):
        raise SystemExit(6)
    guest.append({"permissions":fields[1],"device":fields[3],"inode":int(fields[4]),"path":fields[5].strip()})
if not guest or not any("x" in row["permissions"] for row in guest):
    raise SystemExit(7)
def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1048576), b""):
            h.update(block)
    return h.hexdigest()
guest_sha = digest(binary)
translator_sha = digest("/proc/" + str(pid) + "/exe")
def stable(value):
    return (value.st_dev,value.st_ino,value.st_size,value.st_mtime_ns,value.st_ctime_ns,value.st_mode)
if stable(os.stat(binary)) != stable(source) or int(open("/proc/" + str(pid) + "/stat").read().rpartition(") ")[2].split()[19]) != ticks:
    raise SystemExit(8)
print(json.dumps({"pid":pid,"start_ticks":ticks,"argv":expected,"guest_path":binary,
    "guest_sha256":guest_sha,"guest_dev_major":os.major(source.st_dev),
    "guest_dev_minor":os.minor(source.st_dev),"guest_inode":source.st_ino,
    "guest_size":source.st_size,"guest_mappings":guest,"translator_sha256":translator_sha},sort_keys=True))
"""

_OWNED_WEB_STOP = """import hashlib,json,os,signal,sys,time
token,binary,index,port,wrapper_sha = sys.argv[1:6]
expected_wrapper_pid,expected_wrapper_ticks,expected_child_pid,expected_child_ticks = map(int,sys.argv[6:10])
tombstone = "/tmp/qi-sg-owned-web-" + token + ".cancel"
try:
    marker = os.open(tombstone,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
except FileExistsError:
    try:
        with open(tombstone,"rb") as prior:
            if prior.read(len(token)+1)!=token.encode():
                raise SystemExit(2)
    except OSError:
        raise SystemExit(2)
else:
    try:
        if os.write(marker,token.encode()) != len(token):
            raise SystemExit(3)
        os.fsync(marker)
    finally:
        os.close(marker)
expected_argv = [binary,"-index",index,"-listen","127.0.0.1:"+port,"-html=true"]
def info(pid):
    try:
        raw = open("/proc/"+str(pid)+"/stat").read().rpartition(") ")[2].split()
        argv = open("/proc/"+str(pid)+"/cmdline","rb").read().split(b"\\0")[:-1]
        env = open("/proc/"+str(pid)+"/environ","rb").read().split(b"\\0")
    except OSError:
        return None
    return {"pid":pid,"ticks":int(raw[19]),"state":raw[0],"argv":argv,"env":env}
def kind(row):
    argv=row["argv"]
    if len(argv)==6 and argv==[part.encode() for part in expected_argv] and ("QI_SG_NATIVE_OWNER="+token).encode() in row["env"]:
        return "child"
    if len(argv)==7 and argv[1]==b"-c" and argv[3]==token.encode() and argv[4]==binary.encode() and argv[5]==index.encode() and argv[6]==port.encode():
        if hashlib.sha256(argv[2]).hexdigest()==wrapper_sha:
            return "wrapper"
    return None
def scan():
    found={"child":[],"wrapper":[]}
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        row=info(int(name))
        if row is not None and row["state"]!="Z":
            selected=kind(row)
            if selected:
                found[selected].append(row)
    if any(len(rows)>1 for rows in found.values()):
        raise SystemExit(4)
    return found
def owned(row,which):
    if kind(row)!=which:
        raise SystemExit(5)
    expected=(expected_child_pid,expected_child_ticks) if which=="child" else (expected_wrapper_pid,expected_wrapper_ticks)
    if expected[0] and (row["pid"],row["ticks"])!=expected:
        raise SystemExit(6)
def stop(row,which):
    owned(row,which)
    try:
        fd=os.pidfd_open(row["pid"])
    except ProcessLookupError:
        return
    try:
        current=info(row["pid"])
        if current is None or current["state"]=="Z":
            return
        if current["ticks"]!=row["ticks"]:
            raise SystemExit(7)
        owned(current,which)
        try:
            signal.pidfd_send_signal(fd,signal.SIGKILL)
        except ProcessLookupError:
            pass
    finally:
        os.close(fd)
for _ in range(25):
    current=scan()
    if not current["child"] and not current["wrapper"]:
        break
    time.sleep(0.2)
else:
    for which in ("child","wrapper"):
        for row in current[which]:
            stop(row,which)
for _ in range(25):
    current=scan()
    if not current["child"] and not current["wrapper"]:
        break
    time.sleep(0.2)
else:
    raise SystemExit(8)
for pid,ticks in ((expected_child_pid,expected_child_ticks),(expected_wrapper_pid,expected_wrapper_ticks)):
    if pid:
        row=info(pid)
        if row is not None and row["ticks"]==ticks and row["state"]!="Z":
            raise SystemExit(9)
print(json.dumps({"invocation_id":token,"tombstone_created":True,
                  "owned_child_stopped":True,"owned_supervisor_stopped":True},sort_keys=True))
"""


def _path_stream_inventory(
    stream: bytes,
    *,
    repository: str,
    revision: str,
    expected: set[str],
) -> int:
    """Use the same terminal/path contract while producing and replaying proof."""
    if len(stream) > MAX_STREAM_BYTES:
        raise ValueError("indexed path audit stream exceeds 16 MiB")
    events = sourcegraph._events(stream)
    found: list[str] = []
    progress = None
    for index, (kind, data) in enumerate(events):
        if kind == "done":
            if index != len(events) - 1 or data != {}:
                raise ValueError("indexed path audit invalid terminal event")
        elif kind == "alert":
            raise ValueError("indexed path audit stream alert")
        elif kind == "progress":
            if (
                not isinstance(data, dict)
                or data.get("skipped")
                or type(data.get("done")) is not bool
                or type(data.get("matchCount")) is not int
                or data["matchCount"] < len(found)
                or (
                    progress is not None
                    and (progress["done"] or data["matchCount"] < progress["matchCount"])
                )
            ):
                raise ValueError("indexed path audit incomplete progress")
            progress = data
        elif kind == "matches":
            if (
                not isinstance(data, list)
                or not data
                or (progress is not None and progress["done"])
            ):
                raise ValueError("indexed path audit invalid matches")
            for hit in data:
                if (
                    not isinstance(hit, dict)
                    or hit.get("type") != "path"
                    or hit.get("repository") != repository
                    or hit.get("commit") != revision
                ):
                    raise ValueError("indexed path audit hit identity differs")
                found.append(sourcegraph._path(hit.get("path")))
    if (
        not events
        or events[-1] != ("done", {})
        or progress is None
        or progress["done"] is not True
        or progress["matchCount"] != len(found)
        or len(found) != len(set(found))
        or set(found) != expected
    ):
        raise ValueError("indexed path audit incomplete or foreign inventory")
    return len(found)


def _write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as output:
        output.write(canonical_json(value).encode("utf-8") + b"\n")


def _bounded_path_request(config: dict, query: str) -> tuple[int, str, bytes]:
    """Run the authenticated request in an owned, wall-bounded child process."""
    from tools.benchmark.retrieval import live_lexical_external as live

    child = """import json,sys
sys.path.insert(0, sys.argv[1])
from tools.benchmark.retrieval import live_lexical_external as live
config = json.loads(sys.argv[2]); query = sys.argv[3]
status, content_type, raw, _elapsed = live._http(config, "/.api/search/stream", {"q": query, "v": "V3"}, "text/event-stream", "token")
sys.stdout.buffer.write(json.dumps({"status": status, "content_type": content_type, "bytes": len(raw)}, separators=(",", ":")).encode() + b"\\n" + raw)
"""
    code, output, stderr, _elapsed = live._process(
        [
            sys.executable,
            "-c",
            child,
            str(Path(__file__).resolve().parents[3]),
            json.dumps(config, separators=(",", ":")),
            query,
        ],
        120,
    )
    header, separator, raw = output.partition(b"\n")
    if code != 0 or stderr or not separator:
        raise ValueError("indexed path request process failed or was incomplete")
    metadata = parse_json(header.decode("utf-8", "strict"))
    if (
        not isinstance(metadata, dict)
        or set(metadata) != {"status", "content_type", "bytes"}
        or type(metadata["status"]) is not int
        or not isinstance(metadata["content_type"], str)
        or type(metadata["bytes"]) is not int
        or metadata["bytes"] != len(raw)
    ):
        raise ValueError("indexed path request transport metadata differs")
    return metadata["status"], metadata["content_type"], raw


def _container_binary_sha(container_id: str, binary_path: str) -> str:
    from tools.benchmark.retrieval import live_lexical_external as live

    script = (
        "import hashlib,sys; h=hashlib.sha256(); "
        "f=open(sys.argv[1],'rb'); "
        "[h.update(b) for b in iter(lambda:f.read(1048576),b'')]; "
        "print(h.hexdigest())"
    )
    code, stdout, stderr, _elapsed = live._process(
        ["docker", "exec", container_id, "python3", "-c", script, binary_path], 60
    )
    value = stdout.decode("ascii", "strict").strip()
    if code != 0 or stderr or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("deployed Zoekt binary cannot be bound")
    return value


def _native_server_process(container_id: str, port: int, binary_sha: str | None = None) -> dict:
    """Bind the in-container listening socket to its executable and PID."""
    from tools.benchmark.retrieval import live_lexical_external as live

    script = """import hashlib,json,os,sys
port = int(sys.argv[1]); expected = sys.argv[2]
inodes = set()
for table in ("/proc/net/tcp", "/proc/net/tcp6"):
    try:
        rows = open(table).read().splitlines()[1:]
    except OSError:
        continue
    for line in rows:
        fields = line.split()
        if len(fields) > 9 and int(fields[1].rsplit(":", 1)[1], 16) == port and fields[3] == "0A":
            inodes.add(fields[9])
if not inodes:
    raise SystemExit(10)
if len(inodes) != 1:
    raise SystemExit(11)
owners = []
for name in os.listdir("/proc"):
    if not name.isdigit():
        continue
    directory = "/proc/" + name + "/fd"
    try:
        linked = {os.readlink(directory + "/" + fd) for fd in os.listdir(directory)}
    except OSError:
        continue
    if not any("socket:[" + inode + "]" in linked for inode in inodes):
        continue
    h = hashlib.sha256()
    with open("/proc/" + name + "/exe", "rb") as binary:
        for block in iter(lambda: binary.read(1048576), b""):
            h.update(block)
    stat = open("/proc/" + name + "/stat").read().rpartition(") ")[2].split()
    owners.append({"pid": int(name), "start_ticks": int(stat[19]), "exe_sha256": h.hexdigest()})
if not owners:
    raise SystemExit(12)
if len(owners) != 1:
    raise SystemExit(13)
if expected != "-" and owners[0]["exe_sha256"] != expected:
    raise SystemExit(14)
print(json.dumps({"port": port, **owners[0]}, sort_keys=True))
"""
    code, stdout, stderr, _elapsed = live._process(
        ["docker", "exec", container_id, "python3", "-c", script, str(port), binary_sha or "-"],
        60,
    )
    refusal = {
        10: "native Zoekt listener absent on requested port",
        11: "native Zoekt requested port has ambiguous listening sockets",
        12: "native Zoekt listener socket has no inspectable process owner",
        13: "native Zoekt listener socket has multiple process owners",
        14: "native Zoekt listener executable differs from prior identity",
    }
    if code != 0 or stderr:
        raise ValueError(
            refusal.get(code, "native Zoekt listener process or binary could not be inspected")
        )
    row = parse_json(stdout.decode("utf-8", "strict"))
    if (
        not isinstance(row, dict)
        or set(row) != {"port", "pid", "start_ticks", "exe_sha256"}
        or type(row["port"]) is not int
        or row["port"] != port
        or type(row["pid"]) is not int
        or row["pid"] <= 0
        or type(row["start_ticks"]) is not int
        or row["start_ticks"] <= 0
        or re.fullmatch(r"[0-9a-f]{64}", row["exe_sha256"]) is None
        or (binary_sha is not None and row["exe_sha256"] != binary_sha)
    ):
        raise ValueError("native Zoekt listener process identity differs")
    return row


def _image_guest_binary(container_id: str, image_id: str, binary_path: str) -> None:
    """Require guest bytes from the selected image, without a covering mount or diff."""
    from tools.benchmark.retrieval import live_lexical_external as live

    code, output, stderr, _ = live._process(["docker", "inspect", container_id], 60)
    inspected = parse_json(output.decode("utf-8", "strict")) if code == 0 and not stderr else None
    if (
        not isinstance(inspected, list)
        or len(inspected) != 1
        or inspected[0].get("Id") != container_id
        or inspected[0].get("Image") != image_id
        or not isinstance(inspected[0].get("Mounts"), list)
    ):
        raise ValueError("native Zoekt image identity cannot be bound")
    for mount in inspected[0]["Mounts"]:
        destination = mount.get("Destination") if isinstance(mount, dict) else None
        if not isinstance(destination, str) or not destination.startswith("/"):
            raise ValueError("native Zoekt mount inventory differs")
        if binary_path == destination or binary_path.startswith(destination.rstrip("/") + "/"):
            raise ValueError("native Zoekt guest binary is shadowed by a mount")
    code, output, stderr, _ = live._process(["docker", "diff", container_id], 60)
    if code != 0 or stderr:
        raise ValueError("native Zoekt guest image diff cannot be inspected")
    for line in output.decode("utf-8", "strict").splitlines():
        if len(line) < 4 or line[1:3] != " /" or line[0] not in "ACD":
            raise ValueError("native Zoekt guest image diff is malformed")
        if line[2:] == binary_path:
            raise ValueError("native Zoekt guest binary differs from deployed image")


def _owned_web_guest(
    container_id: str, identity: dict, binary_path: str, index_path: str, port: int
) -> dict:
    from tools.benchmark.retrieval import live_lexical_external as live

    code, output, stderr, _ = live._process(
        [
            "docker",
            "exec",
            container_id,
            "python3",
            "-c",
            _OWNED_WEB_INSPECT,
            str(identity["child_pid"]),
            str(identity["child_start_ticks"]),
            identity["invocation_id"],
            binary_path,
            index_path,
            str(port),
        ],
        60,
    )
    if code != 0 or stderr:
        raise ValueError("native Zoekt guest executable or mapped inode cannot be bound")
    guest = parse_json(output.decode("utf-8", "strict"))
    if (
        not isinstance(guest, dict)
        or set(guest)
        != {
            "pid",
            "start_ticks",
            "argv",
            "guest_path",
            "guest_sha256",
            "guest_dev_major",
            "guest_dev_minor",
            "guest_inode",
            "guest_size",
            "guest_mappings",
            "translator_sha256",
        }
        or guest["pid"] != identity["child_pid"]
        or guest["start_ticks"] != identity["child_start_ticks"]
        or guest["guest_path"] != binary_path
        or guest["guest_sha256"] != identity["guest_sha256"]
        or guest["guest_dev_major"] != identity["guest_dev_major"]
        or guest["guest_dev_minor"] != identity["guest_dev_minor"]
        or guest["guest_inode"] != identity["guest_inode"]
        or guest["guest_size"] != identity["guest_size"]
        or guest["argv"] != identity["argv"]
        or re.fullmatch(r"[0-9a-f]{64}", guest["translator_sha256"]) is None
    ):
        raise ValueError("native Zoekt guest executable identity differs")
    return guest


def _start_owned_web(
    container_id: str, image_id: str, binary_path: str, index_path: str, port: int
) -> tuple[subprocess.Popen, dict]:
    """Start a single invocation-owned read-only webserver, then bind its guest bytes."""
    if not binary_path.startswith("/") or ".." in Path(binary_path).parts:
        raise ValueError("native Zoekt guest binary path is invalid")
    if not index_path.startswith("/") or ".." in Path(index_path).parts:
        raise ValueError("native Zoekt read-only index path is invalid")
    _image_guest_binary(container_id, image_id, binary_path)
    try:
        _native_server_process(container_id, port)
    except ValueError as error:
        if str(error) != "native Zoekt listener absent on requested port":
            raise
    else:
        raise ValueError("native Zoekt owned port is already listening")
    invocation = secrets.token_hex(16)
    process = subprocess.Popen(
        [
            "docker",
            "exec",
            "-i",
            container_id,
            "python3",
            "-c",
            _OWNED_WEB_SUPERVISOR,
            invocation,
            binary_path,
            index_path,
            str(port),
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    if process.stdin is None or process.stdout is None:
        process.kill()
        process.wait()
        raise ValueError("native Zoekt owned service pipes are unavailable")
    identity: dict | None = None
    try:
        os.set_blocking(process.stdout.fileno(), False)
        line = bytearray()
        deadline = time.monotonic() + 30
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while b"\n" not in line:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or len(line) > 8192 or process.poll() is not None:
                    raise ValueError("native Zoekt owned service did not report start identity")
                for _key, _events in selector.select(remaining):
                    chunk = os.read(process.stdout.fileno(), 8193 - len(line))
                    if not chunk:
                        raise ValueError("native Zoekt owned service exited before identity")
                    line.extend(chunk)
        if len(line) > 8192 or line.count(b"\n") != 1:
            raise ValueError("native Zoekt owned service start identity exceeds bound")
        identity = parse_json(bytes(line).decode("utf-8", "strict"))
        expected = {
            "kind",
            "invocation_id",
            "supervisor_pid",
            "supervisor_start_ticks",
            "child_pid",
            "child_start_ticks",
            "guest_path",
            "guest_sha256",
            "guest_dev_major",
            "guest_dev_minor",
            "guest_inode",
            "guest_size",
            "argv",
        }
        if (
            not isinstance(identity, dict)
            or set(identity) != expected
            or identity["kind"] != "owned_zoekt_webserver"
            or identity["invocation_id"] != invocation
            or identity["guest_path"] != binary_path
            or identity["argv"]
            != [binary_path, "-index", index_path, "-listen", f"127.0.0.1:{port}", "-html=true"]
            or any(
                type(identity[key]) is not int or identity[key] <= 0
                for key in (
                    "supervisor_pid",
                    "supervisor_start_ticks",
                    "child_pid",
                    "child_start_ticks",
                    "guest_inode",
                    "guest_size",
                )
            )
            or any(
                type(identity[key]) is not int or identity[key] < 0
                for key in ("guest_dev_major", "guest_dev_minor")
            )
            or re.fullmatch(r"[0-9a-f]{64}", identity["guest_sha256"]) is None
        ):
            raise ValueError("native Zoekt owned service start identity differs")
        listener = None
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise ValueError("native Zoekt owned service exited before listening")
            try:
                listener = _native_server_process(container_id, port)
            except ValueError as error:
                if str(error) != "native Zoekt listener absent on requested port":
                    raise
                time.sleep(0.2)
                continue
            break
        if listener is None:
            raise ValueError("native Zoekt owned service never became a listener")
        if (listener["pid"], listener["start_ticks"]) != (
            identity["child_pid"],
            identity["child_start_ticks"],
        ):
            raise ValueError("native Zoekt listener is not the owned guest child")
        guest = _owned_web_guest(container_id, identity, binary_path, index_path, port)
        if guest["translator_sha256"] != listener["exe_sha256"]:
            raise ValueError("native Zoekt translator process identity differs")
        _image_guest_binary(container_id, image_id, binary_path)
        return process, {**identity, "listener": listener, "guest": guest, "image_id": image_id}
    except BaseException as error:
        cleanup_identity = (
            identity
            if isinstance(identity, dict)
            and all(
                type(identity.get(name)) is int and identity[name] > 0
                for name in (
                    "supervisor_pid",
                    "supervisor_start_ticks",
                    "child_pid",
                    "child_start_ticks",
                )
            )
            else None
        )
        try:
            _stop_owned_web(
                container_id,
                process,
                invocation,
                binary_path,
                index_path,
                port,
                cleanup_identity,
            )
        except BaseException as cleanup_error:
            error.add_note(
                f"Owned Zoekt service cleanup failed: {type(cleanup_error).__name__}: {cleanup_error}"
            )
        raise


def _stop_owned_web(
    container_id: str,
    process: subprocess.Popen,
    invocation: str,
    binary_path: str,
    index_path: str,
    port: int,
    identity: dict | None,
) -> dict:
    """Tombstone first, then prove only this remote supervisor and child exited."""
    from tools.benchmark.retrieval import live_lexical_external as live

    expected = (
        str(identity["supervisor_pid"]) if identity is not None else "0",
        str(identity["supervisor_start_ticks"]) if identity is not None else "0",
        str(identity["child_pid"]) if identity is not None else "0",
        str(identity["child_start_ticks"]) if identity is not None else "0",
    )
    try:
        code, output, stderr, _ = live._process(
            [
                "docker",
                "exec",
                container_id,
                "python3",
                "-c",
                _OWNED_WEB_STOP,
                invocation,
                binary_path,
                index_path,
                str(port),
                sourcegraph.sha256(_OWNED_WEB_SUPERVISOR.encode()),
                *expected,
            ],
            30,
        )
        if code != 0 or stderr:
            raise ValueError("owned native Zoekt service cleanup could not be proved")
        result = parse_json(output.decode("utf-8", "strict"))
        if (
            not isinstance(result, dict)
            or set(result)
            != {
                "invocation_id",
                "tombstone_created",
                "owned_child_stopped",
                "owned_supervisor_stopped",
            }
            or result["invocation_id"] != invocation
            or result["tombstone_created"] is not True
            or result["owned_child_stopped"] is not True
            or result["owned_supervisor_stopped"] is not True
        ):
            raise ValueError("owned native Zoekt service cleanup identity differs")
        return result
    finally:
        if process.stdin is not None:
            process.stdin.close()
        local_cleanup_error = None
        if process.poll() is None:
            try:
                process.kill()
            except ProcessLookupError:
                pass
            except PermissionError as error:
                if process.poll() is None:
                    local_cleanup_error = error
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired as error:
            raise ValueError("owned native Zoekt Docker client cleanup unverified") from error
        finally:
            if process.stdout is not None:
                process.stdout.close()
        if local_cleanup_error is not None:
            raise ValueError(
                "owned native Zoekt Docker client cleanup unverified"
            ) from local_cleanup_error


def _native_worker_row(line: bytes, job: dict) -> tuple[dict, bytes]:
    max_line = (MAX_NATIVE_BODY_BYTES * 4 // 3) + 8192
    if not line or len(line) > max_line:
        raise ValueError("native Zoekt worker response is missing or exceeds bound")
    item = parse_json(line.decode("utf-8", "strict"))
    if not isinstance(item, dict) or set(item) != {
        "repository",
        "path",
        "file_sha256",
        "http_status",
        "actual_sha256",
        "bytes",
        "matches",
        "seconds",
        "body_base64",
    }:
        raise ValueError("native Zoekt worker row shape differs")
    encoded = item.pop("body_base64")
    if not isinstance(encoded, str):
        raise ValueError("native Zoekt worker omitted stored bytes")
    try:
        body = base64.b64decode(encoded, validate=True)
    except binascii.Error as error:
        raise ValueError("native Zoekt worker body encoding differs") from error
    if (
        len(body) > MAX_NATIVE_BODY_BYTES
        or item["repository"] != job["repository"]
        or item["path"] != job["path"]
        or item["file_sha256"] != job["file_sha256"]
        or type(item["http_status"]) is not int
        or item["http_status"] != 200
        or item["matches"] is not True
        or item["actual_sha256"] != sourcegraph.sha256(body)
        or item["actual_sha256"] != job["file_sha256"]
        or type(item["bytes"]) is not int
        or item["bytes"] != len(body)
        or type(item["seconds"]) not in (int, float)
        or not math.isfinite(item["seconds"])
        or item["seconds"] < 0
    ):
        raise ValueError("native Zoekt stored bytes differ from manifest")
    return item, body


def _drain_native_worker(
    argv: list[str],
    jobs: list[dict],
    root: Path,
    *,
    capture_seconds: float = MAX_NATIVE_CAPTURE_SECONDS,
    row_seconds: float = MAX_NATIVE_ROW_SECONDS,
    invocation_id: str | None = None,
) -> tuple[int, dict | None]:
    """Drain only the owned reader process with actual wall and byte bounds."""
    payload = json.dumps(jobs, separators=(",", ":")).encode() + b"\n"
    if len(payload) > MAX_NATIVE_JOBS_BYTES or capture_seconds <= 0 or row_seconds <= 0:
        raise ValueError("native Zoekt worker input or deadline exceeds bound")
    process = subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    if process.stdin is None or process.stdout is None or process.stderr is None:
        process.kill()
        process.wait()
        raise ValueError("native Zoekt worker pipes are unavailable")
    started = time.monotonic()
    deadline = started + capture_seconds
    row_deadline = started + row_seconds
    input_offset = 0
    row_index = 0
    total_body_bytes = 0
    total_stdout_bytes = 0
    stderr = bytearray()
    line_buffer = bytearray()
    max_line = (MAX_NATIVE_BODY_BYTES * 4 // 3) + 8192
    max_stdout = (MAX_TOTAL_NATIVE_BYTES * 4 // 3) + len(jobs) * 8192
    bodies_root = root / "native-file-bodies" / jobs[0]["repository"]
    bodies_root.mkdir(parents=True)
    try:
        for stream in (process.stdin, process.stdout, process.stderr):
            os.set_blocking(stream.fileno(), False)
        with (
            selectors.DefaultSelector() as selector,
            (root / "native-rows.jsonl").open("xb") as rows_file,
        ):
            selector.register(process.stdin, selectors.EVENT_WRITE, "stdin")
            selector.register(process.stdout, selectors.EVENT_READ, "stdout")
            selector.register(process.stderr, selectors.EVENT_READ, "stderr")
            while selector.get_map():
                remaining = min(deadline, row_deadline) - time.monotonic()
                if remaining <= 0:
                    raise ValueError("native Zoekt worker exceeded wall or row deadline")
                for key, _events in selector.select(remaining):
                    stream = key.fileobj
                    if key.data == "stdin":
                        try:
                            written = os.write(
                                stream.fileno(), payload[input_offset : input_offset + 65536]
                            )
                        except BlockingIOError:
                            # Readiness is advisory; retry under the unchanged deadline.
                            continue
                        except BrokenPipeError as error:
                            raise ValueError("native Zoekt worker closed before input") from error
                        if written <= 0:
                            raise ValueError("native Zoekt worker accepted no input")
                        input_offset += written
                        if input_offset == len(payload):
                            selector.unregister(stream)
                            stream.close()
                        continue
                    try:
                        chunk = os.read(stream.fileno(), 65536)
                    except BlockingIOError:
                        continue
                    if not chunk:
                        selector.unregister(stream)
                        continue
                    if key.data == "stderr":
                        stderr.extend(chunk)
                        if len(stderr) > MAX_NATIVE_STDERR_BYTES:
                            raise ValueError("native Zoekt worker stderr exceeds bound")
                        continue
                    total_stdout_bytes += len(chunk)
                    if total_stdout_bytes > max_stdout:
                        raise ValueError("native Zoekt worker total output exceeds bound")
                    line_buffer.extend(chunk)
                    while newline := line_buffer.find(b"\n") + 1:
                        if newline > max_line or row_index >= len(jobs):
                            raise ValueError("native Zoekt worker returned extra or oversized row")
                        line = bytes(line_buffer[:newline])
                        del line_buffer[:newline]
                        item, body = _native_worker_row(line, jobs[row_index])
                        total_body_bytes += len(body)
                        if total_body_bytes > MAX_TOTAL_NATIVE_BYTES:
                            raise ValueError("native Zoekt audit exceeds total byte bound")
                        body_path = bodies_root / jobs[row_index]["path"]
                        body_path.parent.mkdir(parents=True, exist_ok=True)
                        with body_path.open("xb") as body_file:
                            body_file.write(body)
                        rows_file.write(canonical_json(item).encode() + b"\n")
                        row_index += 1
                        row_deadline = time.monotonic() + row_seconds
                    if len(line_buffer) > max_line:
                        raise ValueError("native Zoekt worker unterminated row exceeds bound")
            if line_buffer or row_index != len(jobs) or input_offset != len(payload):
                raise ValueError("native Zoekt worker omitted or truncated rows")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError("native Zoekt worker exceeded wall deadline")
        try:
            code = process.wait(timeout=remaining)
        except subprocess.TimeoutExpired as error:
            raise ValueError("native Zoekt worker exceeded wall deadline") from error
        if code != 0:
            raise ValueError("native Zoekt worker failed")
        identity = None
        if invocation_id is None:
            if stderr:
                raise ValueError("native Zoekt worker wrote unexpected stderr")
        else:
            try:
                identity = parse_json(stderr.decode("utf-8", "strict"))
            except (ValueError, UnicodeDecodeError) as error:
                raise ValueError("native Zoekt worker omitted start identity") from error
            if (
                not isinstance(identity, dict)
                or set(identity) != {"kind", "invocation_id", "pid", "start_ticks"}
                or identity["kind"] != "native_worker_start"
                or identity["invocation_id"] != invocation_id
                or type(identity["pid"]) is not int
                or identity["pid"] <= 0
                or type(identity["start_ticks"]) is not int
                or identity["start_ticks"] <= 0
            ):
                raise ValueError("native Zoekt worker start identity differs")
    except BaseException:
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # Darwin can report EPERM when the short-lived group leader
                # exits between poll and killpg. Reap the owned direct child,
                # then require that its process group is no longer present.
                if process.poll() is None:
                    try:
                        process.kill()
                    except ProcessLookupError:
                        pass
                    except PermissionError as cleanup_error:
                        raise ValueError(
                            "native Zoekt worker direct child cleanup unverified"
                        ) from cleanup_error
        try:
            process.wait(timeout=5)
        except (PermissionError, subprocess.TimeoutExpired) as cleanup_error:
            raise ValueError(
                "native Zoekt worker direct child cleanup unverified"
            ) from cleanup_error
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            pass
        except PermissionError as cleanup_error:
            raise ValueError(
                "native Zoekt worker process group cleanup unverified"
            ) from cleanup_error
        else:
            raise ValueError("native Zoekt worker process group cleanup unverified")
        raise
    finally:
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()
    return row_index, identity


def _cleanup_owned_reader(
    container_id: str,
    invocation_id: str,
    port: int,
    worker_sha: str,
    identity: dict | None,
) -> dict:
    """Inspect and stop only the exact in-container Python reader invocation."""
    from tools.benchmark.retrieval import live_lexical_external as live

    script = """import hashlib,json,os,signal,sys,time
token, port, worker_sha = sys.argv[1:4]
expected_pid, expected_ticks = int(sys.argv[4]), int(sys.argv[5])
cancel_path = "/tmp/qi-sg-owned-reader-" + token + ".cancel"
try:
    cancel_fd = os.open(cancel_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
except FileExistsError:
    raise SystemExit(8)
try:
    os.write(cancel_fd, token.encode())
    os.fsync(cancel_fd)
finally:
    os.close(cancel_fd)
def identity(pid):
    try:
        raw = open("/proc/" + str(pid) + "/stat").read()
    except OSError:
        return None
    fields = raw.rpartition(") ")[2].split()
    return {"start_ticks": int(fields[19]), "state": fields[0]}
def worker_argv(pid):
    try:
        argv = open("/proc/" + str(pid) + "/cmdline", "rb").read().split(b"\\0")
    except OSError:
        return None
    return (len(argv) >= 5 and argv[1] == b"-c" and argv[3] == port.encode()
            and argv[4] == token.encode()
            and hashlib.sha256(argv[2]).hexdigest() == worker_sha)
matches = []
for name in os.listdir("/proc"):
    if not name.isdigit():
        continue
    if worker_argv(int(name)) is not True:
        continue
    observed = identity(int(name))
    if observed is not None:
        matches.append({"pid": int(name), **observed})
if len(matches) > 1:
    raise SystemExit(3)
if matches and expected_pid and (matches[0]["pid"], matches[0]["start_ticks"]) != (expected_pid, expected_ticks):
    raise SystemExit(4)
for row in matches:
    if row["state"] != "Z":
        try:
            process_fd = os.pidfd_open(row["pid"])
        except ProcessLookupError:
            continue
        try:
            current = identity(row["pid"])
            if current is None or current["state"] == "Z":
                continue
            if current["start_ticks"] != row["start_ticks"] or worker_argv(row["pid"]) is not True:
                raise SystemExit(9)
            try:
                signal.pidfd_send_signal(process_fd, signal.SIGKILL)
            except ProcessLookupError:
                pass
        finally:
            os.close(process_fd)
for _ in range(50):
    if all((current := identity(row["pid"])) is None or current["start_ticks"] != row["start_ticks"] or current["state"] == "Z" for row in matches):
        break
    time.sleep(0.1)
else:
    raise SystemExit(5)
if expected_pid:
    current = identity(expected_pid)
    if current is not None and current["start_ticks"] == expected_ticks and current["state"] != "Z":
        raise SystemExit(6)
print(json.dumps({"invocation_id": token, "matched": matches, "stopped": True, "tombstone_created": True}, sort_keys=True))
"""
    expected_pid = identity["pid"] if identity is not None else 0
    expected_ticks = identity["start_ticks"] if identity is not None else 0
    code, output, stderr, _elapsed = live._process(
        [
            "docker",
            "exec",
            container_id,
            "python3",
            "-c",
            script,
            invocation_id,
            str(port),
            worker_sha,
            str(expected_pid),
            str(expected_ticks),
        ],
        30,
    )
    if code != 0 or stderr:
        raise ValueError("owned remote Zoekt reader cleanup could not be proved")
    result = parse_json(output.decode("utf-8", "strict"))
    if (
        not isinstance(result, dict)
        or set(result) != {"invocation_id", "matched", "stopped", "tombstone_created"}
        or result["invocation_id"] != invocation_id
        or result["stopped"] is not True
        or result["tombstone_created"] is not True
        or not isinstance(result["matched"], list)
        or len(result["matched"]) > 1
    ):
        raise ValueError("owned remote Zoekt reader cleanup differs")
    return result


def _native_stored_content(
    root: Path,
    *,
    config: dict,
    manifest_path: Path,
    manifest_raw: bytes,
    files: list[dict],
    repository: str,
    release_digest: str,
    snapshot: dict,
    native_port: int,
    native_binary_path: str | None,
    control_sha256: dict[str, str],
) -> tuple[str, str, dict]:
    """Read bounded raw documents from the current container's Zoekt print API."""
    if native_binary_path is None:
        raise ValueError("native Zoekt owned capture requires an explicit guest binary path")
    container_id = snapshot["runtime"]["container_id"]
    process, owned = _start_owned_web(
        container_id,
        "sha256:" + snapshot["runtime"]["image_sha256"],
        native_binary_path,
        snapshot["runtime"]["mount_destination"],
        native_port,
    )
    try:
        binary_sha, rows_sha = _native_stored_content_running(
            root,
            config=config,
            manifest_path=manifest_path,
            manifest_raw=manifest_raw,
            files=files,
            repository=repository,
            release_digest=release_digest,
            snapshot=snapshot,
            native_port=native_port,
            native_binary_path=native_binary_path,
            control_sha256=control_sha256,
            owned_service=owned,
        )
    except BaseException as error:
        try:
            _stop_owned_web(
                container_id,
                process,
                owned["invocation_id"],
                native_binary_path,
                snapshot["runtime"]["mount_destination"],
                native_port,
                owned,
            )
        except BaseException as cleanup_error:
            error.add_note(
                f"Owned Zoekt service cleanup failed: {type(cleanup_error).__name__}: {cleanup_error}"
            )
        raise
    stopped = _stop_owned_web(
        container_id,
        process,
        owned["invocation_id"],
        native_binary_path,
        snapshot["runtime"]["mount_destination"],
        native_port,
        owned,
    )
    _write_json(root / "owned-service-cleanup.json", stopped)
    return binary_sha, rows_sha, owned


def _capture_translator_bytes(
    root: Path, container_id: str, server_process: dict, invocation_id: str
) -> None:
    """Export a bound proc executable through an owned regular file."""
    from tools.benchmark.retrieval import live_lexical_external as live

    remote = "/tmp/qi-sg-owned-translator-" + invocation_id
    script = """import hashlib,json,os,re,signal,stat,sys
pid,ticks,token,expected,limit=sys.argv[1:]
pid=int(pid);ticks=int(ticks);limit=int(limit)
if not re.fullmatch(r"[0-9a-f]{32}",token) or not re.fullmatch(r"[0-9a-f]{64}",expected):
    raise SystemExit(2)
path="/tmp/qi-sg-owned-translator-"+token
cancel=path+".cancel"
proc="/proc/"+str(pid)
def same():
    try:
        row=open(proc+"/stat").read().rpartition(") ")[2].split()
        return row[0]!="Z" and int(row[19])==ticks
    except (OSError,ValueError,IndexError):
        return False
def deadline(_signum,_frame):
    raise TimeoutError("translator export timed out")
signal.signal(signal.SIGALRM,deadline)
signal.setitimer(signal.ITIMER_REAL,50)
if os.path.exists(cancel) or not same():
    raise SystemExit(3)
process_fd=os.pidfd_open(pid)
created=False
try:
    with open(proc+"/exe","rb",buffering=0) as source:
        initial=os.fstat(source.fileno())
        if not stat.S_ISREG(initial.st_mode) or not 0<initial.st_size<=limit:
            raise SystemExit(4)
        if os.path.exists(cancel):
            raise SystemExit(3)
        fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
        created=True
        h=hashlib.sha256();total=0
        with os.fdopen(fd,"wb") as output:
            if os.path.exists(cancel):
                raise SystemExit(3)
            while block:=source.read(1048576):
                if os.path.exists(cancel):
                    raise SystemExit(3)
                total+=len(block)
                if total>limit:
                    raise SystemExit(5)
                output.write(block);h.update(block)
            output.flush();os.fsync(output.fileno())
        after=os.fstat(source.fileno())
    written=os.stat(path,follow_symlinks=False)
    if (total!=initial.st_size or total!=after.st_size or h.hexdigest()!=expected
            or not same() or os.path.exists(cancel) or written.st_size!=total
            or not stat.S_ISREG(written.st_mode)):
        raise SystemExit(6)
    print(json.dumps({"path":path,"sha256":h.hexdigest(),"bytes":total,
        "device":written.st_dev,"inode":written.st_ino,"pid":pid,"start_ticks":ticks},sort_keys=True))
    created=False
finally:
    if created:
        try: os.unlink(path)
        except FileNotFoundError: pass
    os.close(process_fd)
"""
    cleanup = """import hashlib,json,os,re,signal,stat,sys,time
token,script_sha,limit=sys.argv[1:]
limit=int(limit)
if not re.fullmatch(r"[0-9a-f]{32}",token) or not re.fullmatch(r"[0-9a-f]{64}",script_sha):
    raise SystemExit(2)
path="/tmp/qi-sg-owned-translator-"+token
cancel=path+".cancel"
try:
    marker=os.open(cancel,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
except FileExistsError:
    with open(cancel,"rb") as prior:
        if prior.read()!=token.encode():
            raise SystemExit(3)
else:
    try:
        if os.write(marker,token.encode())!=len(token):
            raise SystemExit(4)
        os.fsync(marker)
    finally: os.close(marker)
def workers():
    found=[]
    for name in os.listdir("/proc"):
        if not name.isdigit(): continue
        try:
            argv=open("/proc/"+name+"/cmdline","rb").read().split(b"\\0")[:-1]
            if (len(argv)==8 and argv[1]==b"-c"
                    and hashlib.sha256(argv[2]).hexdigest()==script_sha
                    and argv[5]==token.encode()):
                state=open("/proc/"+name+"/stat").read().rpartition(") ")[2].split()[0]
                if state!="Z": found.append(int(name))
        except (OSError,IndexError): pass
    return found
def remove():
    try: fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    except FileNotFoundError: return
    try:
        row=os.fstat(fd)
        if (not stat.S_ISREG(row.st_mode) or row.st_uid!=os.geteuid()
                or row.st_nlink!=1 or row.st_size>limit):
            raise SystemExit(5)
        current=os.stat(path,follow_symlinks=False)
        if (current.st_dev,current.st_ino)!=(row.st_dev,row.st_ino):
            raise SystemExit(7)
        os.unlink(path)
    finally: os.close(fd)
end=time.monotonic()+65
while True:
    remove()
    active=workers()
    if not active:
        remove()
        if not os.path.lexists(path) and not workers(): break
    if time.monotonic()>=end: raise SystemExit(6)
    time.sleep(0.1)
print(json.dumps({"tombstone_created":True,"removed":not os.path.lexists(path),
    "owned_worker_stopped":True,"path":path},sort_keys=True))
"""
    error = None
    try:
        code, output, stderr, _ = live._process(
            [
                "docker",
                "exec",
                container_id,
                "python3",
                "-c",
                script,
                str(server_process["pid"]),
                str(server_process["start_ticks"]),
                invocation_id,
                server_process["exe_sha256"],
                str(MAX_TRANSLATOR_BYTES),
            ],
            60,
        )
        if code != 0 or stderr:
            raise ValueError("native Zoekt translator proc executable export failed")
        exported = parse_json(output.decode("utf-8", "strict"))
        if (
            not isinstance(exported, dict)
            or set(exported) != {"path", "sha256", "bytes", "device", "inode", "pid", "start_ticks"}
            or exported["path"] != remote
            or exported["sha256"] != server_process["exe_sha256"]
            or exported["pid"] != server_process["pid"]
            or exported["start_ticks"] != server_process["start_ticks"]
            or any(
                type(exported[key]) is not int or exported[key] <= 0
                for key in ("bytes", "device", "inode")
            )
            or exported["bytes"] > MAX_TRANSLATOR_BYTES
        ):
            raise ValueError("native Zoekt translator export identity differs")
        copied, _stdout, _stderr, _ = live._process(
            ["docker", "cp", f"{container_id}:{remote}", str(root / "native-translator")],
            60,
        )
        if (
            copied != 0
            or not (root / "native-translator").is_file()
            or (root / "native-translator").is_symlink()
            or (root / "native-translator").stat().st_size != exported["bytes"]
            or RawFile.capture(root / "native-translator").sha256
            != "sha256:" + server_process["exe_sha256"]
        ):
            raise ValueError("native Zoekt translator executable bytes differ")
    except BaseException as caught:
        error = caught
    finally:
        # Cancellation precedes the scan. A delayed exec sees the tombstone
        # before creation or immediately after O_EXCL and removes only its file.
        try:
            code, output, stderr, _ = live._process(
                [
                    "docker",
                    "exec",
                    container_id,
                    "python3",
                    "-c",
                    cleanup,
                    invocation_id,
                    sourcegraph.sha256(script.encode()),
                    str(MAX_TRANSLATOR_BYTES),
                ],
                80,
            )
            result = (
                parse_json(output.decode("utf-8", "strict")) if code == 0 and not stderr else None
            )
            if result != {
                "tombstone_created": True,
                "removed": True,
                "owned_worker_stopped": True,
                "path": remote,
            }:
                raise ValueError("native Zoekt translator export cleanup is unverified")
        except BaseException as cleanup_error:
            raise ValueError(
                "native Zoekt translator export cleanup is unverified"
            ) from cleanup_error
    if error is not None:
        raise error


def _native_stored_content_running(
    root: Path,
    *,
    config: dict,
    manifest_path: Path,
    manifest_raw: bytes,
    files: list[dict],
    repository: str,
    release_digest: str,
    snapshot: dict,
    native_port: int,
    native_binary_path: str,
    control_sha256: dict[str, str],
    owned_service: dict,
) -> tuple[str, str]:
    from tools.benchmark.retrieval import live_lexical_external as live

    runtime = snapshot["runtime"]
    container_id = runtime["container_id"]
    native = {
        "container_id": container_id,
        "image_id": "sha256:" + runtime["image_sha256"],
        "started_at": runtime["started_at"],
        "restart_count": runtime["restart_count"],
        "pid": runtime["pid"],
        "mounts": [
            {
                "Type": "bind",
                "Source": runtime["mount_source"],
                "Destination": runtime["mount_destination"],
                "RW": False,
            }
        ],
    }
    native_index = {"runtime": native, "files": snapshot["files"]}
    _write_json(root / "native-index-before.json", native_index)
    server_process = owned_service["listener"]
    binary_sha = owned_service["guest"]["guest_sha256"]
    binary_source = native_binary_path
    copied_code, _copied_stdout, _copied_stderr, _elapsed = live._process(
        ["docker", "cp", "-L", f"{container_id}:{binary_source}", str(root / "zoekt-webserver")],
        60,
    )
    binary_file = root / "zoekt-webserver"
    if (
        copied_code != 0
        or not binary_file.is_file()
        or binary_file.is_symlink()
        or RawFile.capture(binary_file).sha256 != "sha256:" + binary_sha
    ):
        raise ValueError("deployed Zoekt binary bytes differ")
    _capture_translator_bytes(root, container_id, server_process, owned_service["invocation_id"])
    with (root / "native-worker.py").open("x", encoding="utf-8") as worker_file:
        worker_file.write(_NATIVE_WORKER)
    script_file = root / "probe_native_contents.py"
    producer_source = Path(__file__).read_bytes()
    with script_file.open("xb") as script_output:
        script_output.write(producer_source)
    jobs = [{"repository": repository, **row} for row in files]
    invocation_id = secrets.token_hex(16)
    worker_sha = sourcegraph.sha256(_NATIVE_WORKER.encode())
    _write_json(
        root / "precommit.json",
        {
            "scope": "Zoekt indexed document payload, not gitserver source",
            "qualified": False,
            "manifest_sha256": {str(manifest_path): sourcegraph.sha256(manifest_raw)},
            "control_sha256": control_sha256,
            "script_sha256": sourcegraph.sha256(script_file.read_bytes()),
            "worker_sha256": worker_sha,
            "reader_invocation_id": invocation_id,
            "native_binary_sha256": binary_sha,
            "native_server_process": server_process,
            "native_owned_service": owned_service,
            "native_runtime": native,
            "tasks": len(jobs),
            "worker_concurrency": 1,
        },
    )
    try:
        rows, reader_identity = _drain_native_worker(
            [
                "docker",
                "exec",
                "-i",
                container_id,
                "python3",
                "-c",
                _NATIVE_WORKER,
                str(native_port),
                invocation_id,
            ],
            jobs,
            root,
            invocation_id=invocation_id,
        )
    except BaseException as worker_error:
        try:
            _cleanup_owned_reader(container_id, invocation_id, native_port, worker_sha, None)
        except (OSError, ValueError, subprocess.SubprocessError) as cleanup_error:
            raise ValueError(
                "native reader failed and remote cleanup is unverified"
            ) from cleanup_error
        raise worker_error
    cleanup = _cleanup_owned_reader(
        container_id, invocation_id, native_port, worker_sha, reader_identity
    )
    current = live._backend_snapshot(config)
    if (
        rows != len(files)
        or _container_binary_sha(container_id, native_binary_path) != binary_sha
        or _native_server_process(container_id, native_port) != server_process
        or _owned_web_guest(
            container_id,
            owned_service,
            native_binary_path,
            runtime["mount_destination"],
            native_port,
        )
        != owned_service["guest"]
        or current != snapshot
        or Path(__file__).read_bytes() != producer_source
    ):
        raise ValueError("native Zoekt worker coverage, binary or producer changed")
    _image_guest_binary(container_id, owned_service["image_id"], native_binary_path)
    current_runtime = current["runtime"]
    after_native = {
        "runtime": {
            **native,
            "container_id": current_runtime["container_id"],
            "image_id": "sha256:" + current_runtime["image_sha256"],
            "started_at": current_runtime["started_at"],
            "restart_count": current_runtime["restart_count"],
            "pid": current_runtime["pid"],
        },
        "files": current["files"],
    }
    _write_json(root / "native-index-after.json", after_native)
    rows_sha = sourcegraph.sha256((root / "native-rows.jsonl").read_bytes())
    _write_json(
        root / "owned-probe-cleanup.json",
        {
            "deployed_binary_sha256": binary_sha,
            "only_owned_reader_stopped": True,
            "reader_invocation_id": invocation_id,
            "reader_identity": reader_identity,
            "remote_cleanup": cleanup,
        },
    )
    _write_json(
        root / "result.json",
        {
            "status": "VERIFIED",
            "qualified": False,
            "bindings_unchanged": True,
            "failures": [],
            "worker_exit": 0,
            "release_digest": release_digest,
            "rows_sha256": rows_sha,
            "control_sha256": control_sha256,
            "native_server_process": server_process,
            "native_owned_service": owned_service,
            "reader_invocation_id": invocation_id,
            "reader_identity": reader_identity,
            "expected_files": len(files),
            "observed_files": rows,
            "matched_files": rows,
        },
    )
    return binary_sha, rows_sha


def _capture_spec(path: Path, *, scope_spec: bool, live) -> dict:
    if scope_spec:
        selected = parse_json(_read_control_file(path).decode("utf-8"))
        if (
            not isinstance(selected, dict)
            or set(selected) != {"schema_version", "corpus", "sourcegraph"}
            or selected["schema_version"] != 1
        ):
            raise ValueError("Sourcegraph index scope input spec differs")
        live.corpus_binding._selection(selected["corpus"])
        selected["sourcegraph"] = live._service(
            selected["sourcegraph"],
            {"base_url", "repository", "server_image_digest"},
            {"backend_snapshot", "projection_git_root"},
        )
        return selected
    selected = live._spec(path)
    if "sourcegraph" not in live._selected_products(selected):
        raise ValueError("index scope capture requires a selected Sourcegraph product")
    return selected


def _early_control_sha256(path: Path, spec: dict, *, scope_spec: bool, live) -> dict[str, str]:
    from tools.benchmark.retrieval import lexical_file_comparison as lexical

    config = spec["sourcegraph"]
    paths = (
        path,
        *((Path(spec["suite"]), Path(spec["query_pack"])) if not scope_spec else ()),
        Path(config["token_file"]),
        Path(__file__),
        Path(live.__file__),
        Path(sourcegraph.__file__),
        Path(live.corpus_binding.__file__),
        Path(live.corpus_release.__file__),
        Path(lexical.__file__),
    )
    controls = {
        str(item.resolve(strict=True)): RawFile.capture(item).sha256.removeprefix("sha256:")
        for item in paths
    }
    if len(controls) != len(paths):
        raise ValueError("index scope control files overlap")
    return controls


def _unchanged_controls(controls: dict[str, str]) -> bool:
    return all(
        RawFile.capture(Path(path)).sha256 == "sha256:" + digest
        for path, digest in controls.items()
    )


def capture_from_live_spec(
    live_spec_path: Path,
    output_root: Path,
    *,
    native_port: int,
    native_binary_path: str | None = None,
    scope_spec: bool = False,
    bound_release=None,
    preflight_controls: dict[str, str] | None = None,
) -> Path:
    """Produce the existing v1 scope receipt from a live Sourcegraph/Zoekt service.

    The live query spec supplies the corpus, authenticated Sourcegraph endpoint,
    exact projection, and Docker/index authority. The caller supplies the
    observed in-container Zoekt print port; the listener executable is read
    from its procfs PID. An optional binary path is checked against that PID.
    No receipt is issued if any
    request, native body, or before/after identity is incomplete.
    """
    from tools.benchmark.retrieval import lexical_file_comparison as lexical
    from tools.benchmark.retrieval import live_lexical_external as live

    corpus_binding = live.corpus_binding
    corpus_release = live.corpus_release

    if type(native_port) is not int or not 1 <= native_port <= 65535:
        raise ValueError("native Zoekt print port is invalid")
    if native_binary_path is not None and (
        not isinstance(native_binary_path, str)
        or not native_binary_path.startswith("/")
        or ".." in Path(native_binary_path).parts
    ):
        raise ValueError("native Zoekt binary path must be canonical absolute")
    output_root = _absolute(str(output_root))
    spec_before = RawFile.capture(live_spec_path)
    spec = _capture_spec(live_spec_path, scope_spec=scope_spec, live=live)
    if native_binary_path is None:
        raise ValueError("native Zoekt owned capture requires explicit guest binary path")
    config = spec["sourcegraph"]
    if (
        "backend_snapshot" not in config
        or "projection_git_root" not in config
        or "token_file" not in config
        or "indexed_scope_receipt" in config
    ):
        raise ValueError("index scope capture requires fresh backend, projection and token inputs")
    release = Path(spec["corpus"]["release_path"])
    early_controls = _early_control_sha256(live_spec_path, spec, scope_spec=scope_spec, live=live)
    if (preflight_controls is not None and early_controls != preflight_controls) or RawFile.capture(
        live_spec_path
    ) != spec_before:
        raise ValueError("index scope preflight control files changed")
    driver_root = Path(__file__).resolve().parents[3]
    input_paths = [
        live_spec_path,
        release,
        Path(config["backend_snapshot"]["root"]),
        Path(config["projection_git_root"]),
    ]
    if not scope_spec:
        input_paths.extend((Path(spec["suite"]), Path(spec["query_pack"])))
    for input_path in input_paths:
        if input_path.resolve().is_relative_to(driver_root):
            raise ValueError("index scope inputs must stay outside the driver checkout")
    if (
        output_root.exists()
        or output_root.is_symlink()
        or output_root.resolve().is_relative_to(driver_root)
        or any(
            output_root.resolve().is_relative_to(input_path.resolve())
            or input_path.resolve().is_relative_to(output_root.resolve())
            for input_path in (
                release,
                *((Path(spec["output_root"]),) if not scope_spec else ()),
                Path(config["backend_snapshot"]["root"]),
                Path(config["projection_git_root"]),
            )
        )
    ):
        raise ValueError("index scope output must be fresh and disjoint")
    bound = bound_release if bound_release is not None else live.BoundRelease.begin(release)
    document = bound.recheck(release)
    if not _unchanged_controls(early_controls):
        raise ValueError("index scope preflight control files changed")
    if document["digest"] != spec["corpus"]["release_digest"]:
        raise ValueError("index scope release digest differs")
    repo = spec["corpus"]["repository"]
    matches = [row for row in document["repositories"] if row["recipe"]["name"] == repo]
    if len(matches) != 1 or config["repository"] != "benchmark/" + repo:
        raise ValueError("index scope selected repository differs")
    view = spec["corpus"]["view"]
    manifest_path = release / matches[0]["views"][view]["manifest"]
    manifest_raw = _read_control_file(manifest_path)
    manifest = parse_json(manifest_raw.decode("utf-8"))
    files = sourcegraph._files(manifest["files"], "index scope manifest")
    if not scope_spec:
        suite_raw = _read_control_file(Path(spec["suite"]))
        pack_raw = _read_control_file(Path(spec["query_pack"]))
        corpus_binding._bind(document, manifest_raw, spec["corpus"], suite_raw, pack_raw)
        if set(row["path"] for row in files) != lexical._file_universe(
            parse_json(suite_raw.decode()), parse_json(pack_raw.decode())
        ):
            raise ValueError("index scope manifest differs from live query universe")
    projection = live._projection_binding(config, manifest)
    if projection is None or projection["source_revision"] != manifest["repository_commit"]:
        raise ValueError("index scope projection differs from source")
    control_paths = (
        live_spec_path,
        release / "release.json",
        manifest_path,
        *((Path(spec["suite"]), Path(spec["query_pack"])) if not scope_spec else ()),
        Path(config["token_file"]),
        Path(__file__),
        Path(live.__file__),
        Path(sourcegraph.__file__),
        Path(corpus_binding.__file__),
        Path(corpus_release.__file__),
        Path(lexical.__file__),
    )
    controls = {
        str(path.resolve(strict=True)): RawFile.capture(path).sha256.removeprefix("sha256:")
        for path in control_paths
    }
    if len(controls) != len(control_paths):
        raise ValueError("index scope control files overlap")
    if any(controls.get(path) != digest for path, digest in early_controls.items()):
        raise ValueError("index scope preflight control files changed")
    before = live._backend_snapshot(config)
    live._validate_backend_snapshot(config, before)
    output_root.mkdir(parents=True)
    path_root = output_root / "path-audit"
    native_root = output_root / "native-audit"
    path_root.mkdir()
    native_root.mkdir()
    _write_json(path_root / "native-index-before.json", before)
    revision = projection["projection_revision"]
    query = f".* repo:^benchmark/{repo}$ rev:{revision} type:path patternType:regexp count:all"
    status, content_type, stream = _bounded_path_request(config, query)
    if status != 200 or content_type != "text/event-stream":
        raise ValueError("indexed path audit HTTP response differs")
    count = _path_stream_inventory(
        stream,
        repository=config["repository"],
        revision=revision,
        expected={row["path"] for row in files},
    )
    with (path_root / f"{repo}.stream").open("xb") as stream_file:
        stream_file.write(stream)
    path_after = live._backend_snapshot(config)
    live._validate_backend_snapshot(config, path_after)
    _write_json(path_root / "native-index-after.json", path_after)
    if path_after != before:
        raise ValueError("index or process changed during path inventory")
    _write_json(
        path_root / "summary.json",
        {
            "qualified": False,
            "status": "path_index_observed",
            "release_digest": document["digest"],
            "server_image_digest": config["server_image_digest"],
            "native_index_sha256": before["tree_sha256"],
            "repositories": [
                {
                    "repository": repo,
                    "source_commit": manifest["repository_commit"],
                    "projection_commit": revision,
                    "files": count,
                    "native_match_count": count,
                    "raw_stream_sha256": sourcegraph.sha256(stream),
                    "query": query,
                }
            ],
        },
    )
    binary_sha, rows_sha, owned_service = _native_stored_content(
        native_root,
        config=config,
        manifest_path=manifest_path,
        manifest_raw=manifest_raw,
        files=files,
        repository=repo,
        release_digest=document["digest"],
        snapshot=before,
        native_port=native_port,
        native_binary_path=native_binary_path,
        control_sha256=controls,
    )
    after = live._backend_snapshot(config)
    live._validate_backend_snapshot(config, after)
    if after != before:
        raise ValueError("index or process changed during native body audit")
    if RawFile.capture(live_spec_path) != spec_before:
        raise ValueError("live Sourcegraph capture spec changed during native audit")
    if (
        {name: RawFile.capture(Path(name)).sha256.removeprefix("sha256:") for name in controls}
        != controls
        or bound.recheck(release) != document
        or live._projection_binding(config, manifest) != projection
    ):
        raise ValueError("index scope source or control files changed during native audit")
    native_before_path = native_root / "native-index-before.json"
    receipt = native_root / "receipt.json"
    _write_json(
        receipt,
        {
            "schema": "external_index_scope_v1",
            "backend": "sourcegraph_zoekt",
            "scope": SCOPE,
            "qualified_comparison": False,
            "release_digest": document["digest"],
            "repository": repo,
            "repository_commit": manifest["repository_commit"],
            "corpus_manifest_sha256": sourcegraph.sha256(manifest_raw),
            "files": files,
            "native_index_inventory_sha256": sourcegraph.sha256(native_before_path.read_bytes()),
            "native_runtime": parse_json(native_before_path.read_text())["runtime"],
            "native_binary_sha256": binary_sha,
            "native_owned_service": owned_service,
            "native_capture_root": str(native_root),
            "native_rows_sha256": rows_sha,
            "path_inventory_proof": str(path_root / "summary.json"),
            "limitations": [
                "No assertion of every posting's correctness",
                "No human qrels claim",
                "No speed qualification",
            ],
        },
    )
    verify(
        receipt,
        manifest_raw=manifest_raw,
        release_digest=document["digest"],
        config=config,
        projection=projection,
        snapshot=before,
        require_owned_service=True,
    )
    if bound.recheck(release) != document:
        raise ValueError("index scope release changed during evidence replay")
    return receipt


def _absolute(value: object) -> Path:
    if not isinstance(value, str):
        raise ValueError("index scope path must be a canonical absolute string")
    path = Path(value)
    if not path.is_absolute() or str(path) != value or ".." in path.parts:
        raise ValueError("index scope path must be canonical absolute")
    return path


def capture_scope_batch(
    batch_spec_path: Path,
    *,
    native_port: int,
    native_binary_path: str | None = None,
) -> list[Path]:
    """Reuse one bound corpus release across independent v1 scope receipts."""
    from tools.benchmark.retrieval import live_lexical_external as live

    if type(native_port) is not int or not 1 <= native_port <= 65535:
        raise ValueError("native Zoekt print port is invalid")
    if native_binary_path is not None and (
        not isinstance(native_binary_path, str)
        or not native_binary_path.startswith("/")
        or ".." in Path(native_binary_path).parts
    ):
        raise ValueError("native Zoekt binary path must be canonical absolute")
    driver_root = Path(__file__).resolve().parents[3]
    batch_before = RawFile.capture(batch_spec_path)
    if batch_spec_path.resolve().is_relative_to(driver_root):
        raise ValueError("index scope batch inputs must stay outside the driver checkout")
    batch = parse_json(_read_control_file(batch_spec_path).decode("utf-8"))
    if (
        not isinstance(batch, dict)
        or set(batch) != {"schema_version", "cells"}
        or batch["schema_version"] != 1
        or not isinstance(batch["cells"], list)
        or not batch["cells"]
    ):
        raise ValueError("Sourcegraph index scope batch input differs")
    if native_binary_path is None:
        raise ValueError("native Zoekt owned capture requires explicit guest binary path")
    cells = []
    release_root = None
    repositories: set[str] = set()
    inputs = {batch_spec_path.resolve(strict=True)}
    outputs: list[Path] = []
    for row in batch["cells"]:
        if not isinstance(row, dict) or set(row) != {"scope_spec", "output_root"}:
            raise ValueError("Sourcegraph index scope batch cell differs")
        path = _absolute(row["scope_spec"])
        output = _absolute(row["output_root"])
        if path.resolve().is_relative_to(driver_root) or output.resolve().is_relative_to(
            driver_root
        ):
            raise ValueError(
                "index scope batch inputs and outputs must stay outside the driver checkout"
            )
        spec = _capture_spec(path, scope_spec=True, live=live)
        config = spec["sourcegraph"]
        if (
            "backend_snapshot" not in config
            or "projection_git_root" not in config
            or "token_file" not in config
            or "indexed_scope_receipt" in config
        ):
            raise ValueError(
                "index scope batch requires fresh backend, projection and token inputs"
            )
        repo = spec["corpus"]["repository"]
        release = Path(spec["corpus"]["release_path"])
        resolved_release = release.resolve(strict=True)
        if repo in repositories or (release_root is not None and release_root != resolved_release):
            raise ValueError("index scope batch has duplicate repository or different release root")
        repositories.add(repo)
        release_root = resolved_release
        early = _early_control_sha256(path, spec, scope_spec=True, live=live)
        inputs.update(Path(name) for name in early)
        inputs.update(
            (
                resolved_release,
                Path(config["backend_snapshot"]["root"]).resolve(strict=True),
                Path(config["projection_git_root"]).resolve(strict=True),
            )
        )
        cells.append((path, output, release, early))
        outputs.append(output.resolve())
    if any(
        output.exists()
        or output.is_symlink()
        or any(
            output == item or output.is_relative_to(item) or item.is_relative_to(output)
            for item in inputs
        )
        or any(
            output == other or output.is_relative_to(other) or other.is_relative_to(output)
            for other in outputs[index + 1 :]
        )
        for index, output in enumerate(outputs)
    ):
        raise ValueError("index scope batch outputs must be fresh and disjoint")

    def recheck_inputs() -> None:
        if RawFile.capture(batch_spec_path) != batch_before or any(
            not _unchanged_controls(early) for _, _, _, early in cells
        ):
            raise ValueError("index scope batch input or control files changed")

    recheck_inputs()
    bound = live.BoundRelease.begin(release_root)
    recheck_inputs()
    receipts = []
    for path, output, _release, early in cells:
        recheck_inputs()
        receipts.append(
            capture_from_live_spec(
                path,
                output,
                native_port=native_port,
                native_binary_path=native_binary_path,
                scope_spec=True,
                bound_release=bound,
                preflight_controls=early,
            )
        )
        recheck_inputs()
    return receipts


def verify(
    receipt_path: Path,
    *,
    manifest_raw: bytes,
    release_digest: str,
    config: dict,
    projection: dict,
    snapshot: dict,
    require_owned_service: bool = False,
) -> dict:
    try:
        return _verify(
            receipt_path,
            manifest_raw=manifest_raw,
            release_digest=release_digest,
            config=config,
            projection=projection,
            snapshot=snapshot,
            require_owned_service=require_owned_service,
        )
    except (KeyError, TypeError, UnicodeDecodeError) as exc:
        raise ValueError("malformed index scope evidence") from exc


def _verify(
    receipt_path: Path,
    *,
    manifest_raw: bytes,
    release_digest: str,
    config: dict,
    projection: dict,
    snapshot: dict,
    require_owned_service: bool,
) -> dict:
    """Re-derive one repository scope; never trust a receipt's success flag."""
    commitments: dict[str, RawFile] = {}

    def raw(path: Path) -> RawFile:
        item = RawFile.capture(path)
        commitments[str(item.path)] = item
        return item

    def read(path: Path, limit: int | None = None) -> bytes:
        item = raw(path)
        value = _read_control_file(path, max_bytes=limit)
        if sourcegraph.sha256(value) != item.sha256.removeprefix("sha256:"):
            raise ValueError("index scope bytes changed during read")
        return value

    def document(path: Path) -> dict:
        value = parse_json(read(path).decode("utf-8"))
        if not isinstance(value, dict):
            raise ValueError("index scope document must be an object")
        return value

    receipt = document(receipt_path)
    base_receipt_keys = {
        "schema",
        "backend",
        "scope",
        "qualified_comparison",
        "release_digest",
        "repository",
        "repository_commit",
        "corpus_manifest_sha256",
        "files",
        "native_index_inventory_sha256",
        "native_runtime",
        "native_binary_sha256",
        "native_capture_root",
        "native_rows_sha256",
        "path_inventory_proof",
        "limitations",
    }
    if set(receipt) not in (base_receipt_keys, base_receipt_keys | {"native_owned_service"}):
        raise ValueError("index scope receipt keys differ")
    manifest = parse_json(manifest_raw.decode("utf-8"))
    files = sourcegraph._files(manifest["files"], "index scope manifest")
    repo = receipt["repository"]
    if (
        receipt["schema"] != "external_index_scope_v1"
        or receipt["backend"] != "sourcegraph_zoekt"
        or receipt["scope"] != SCOPE
        or receipt["qualified_comparison"] is not False
        or receipt["release_digest"] != release_digest
        or receipt["corpus_manifest_sha256"] != sourcegraph.sha256(manifest_raw)
        or receipt["repository_commit"] != manifest["repository_commit"]
        or receipt["files"] != files
        or not isinstance(repo, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", repo) is None
        or config["repository"] != "benchmark/" + repo
        or projection is None
        or projection["source_revision"] != manifest["repository_commit"]
        or "backend_snapshot" not in config
        or receipt["limitations"]
        != [
            "No assertion of every posting's correctness",
            "No human qrels claim",
            "No speed qualification",
        ]
    ):
        raise ValueError("index scope receipt differs from selected source or contract")
    root = _absolute(receipt["native_capture_root"])
    before = document(root / "native-index-before.json")
    after = document(root / "native-index-after.json")
    if (
        before != after
        or set(before) != {"runtime", "files"}
        or commitments[str(root / "native-index-before.json")].sha256
        != "sha256:" + receipt["native_index_inventory_sha256"]
        or before["files"] != snapshot["files"]
        or before["runtime"] != receipt["native_runtime"]
    ):
        raise ValueError("native content audit index differs from query capture")
    native = before["runtime"]
    runtime = snapshot["runtime"]
    mounts = native.get("mounts")
    if (
        native.get("container_id") != runtime["container_id"]
        or native.get("image_id") != "sha256:" + runtime["image_sha256"]
        or type(native.get("pid")) is not int
        or native["pid"] != runtime["pid"]
        or native.get("started_at") != runtime["started_at"]
        or type(native.get("restart_count")) is not int
        or native["restart_count"] != runtime["restart_count"]
        or not isinstance(mounts, list)
        or len(mounts) != 1
        or not isinstance(mounts[0], dict)
        or mounts[0].get("Type") != "bind"
        or mounts[0].get("RW") is not False
        or mounts[0].get("Source") != runtime["mount_source"]
        or mounts[0].get("Destination") != runtime["mount_destination"]
    ):
        raise ValueError("native content audit runtime differs from query capture")
    precommit = document(root / "precommit.json")
    result = document(root / "result.json")
    cleanup = document(root / "owned-probe-cleanup.json")
    owned = receipt.get("native_owned_service")
    if require_owned_service and owned is None:
        raise ValueError("native Zoekt owned guest proof is required")
    if "native_owned_service" in receipt and owned is None:
        raise ValueError("native Zoekt owned service receipt differs")
    if owned is not None:
        if (
            not isinstance(owned, dict)
            or set(owned)
            != {
                "kind",
                "invocation_id",
                "supervisor_pid",
                "supervisor_start_ticks",
                "child_pid",
                "child_start_ticks",
                "guest_path",
                "guest_sha256",
                "guest_dev_major",
                "guest_dev_minor",
                "guest_inode",
                "guest_size",
                "argv",
                "listener",
                "guest",
                "image_id",
            }
            or owned["kind"] != "owned_zoekt_webserver"
            or not isinstance(owned["invocation_id"], str)
            or re.fullmatch(r"[0-9a-f]{32}", owned["invocation_id"]) is None
            or owned["image_id"] != native["image_id"]
            or any(
                type(owned[name]) is not int or owned[name] <= 0
                for name in (
                    "supervisor_pid",
                    "supervisor_start_ticks",
                    "child_pid",
                    "child_start_ticks",
                    "guest_inode",
                    "guest_size",
                )
            )
            or any(
                type(owned[name]) is not int or owned[name] < 0
                for name in ("guest_dev_major", "guest_dev_minor")
            )
            or not isinstance(owned["guest_path"], str)
            or not owned["guest_path"].startswith("/")
            or ".." in Path(owned["guest_path"]).parts
            or precommit.get("native_owned_service") != owned
            or result.get("native_owned_service") != owned
        ):
            raise ValueError("native Zoekt owned service receipt differs")
        guest = owned["guest"]
        listener = owned["listener"]
        mappings = guest.get("guest_mappings") if isinstance(guest, dict) else None
        if (
            not isinstance(guest, dict)
            or set(guest)
            != {
                "pid",
                "start_ticks",
                "argv",
                "guest_path",
                "guest_sha256",
                "guest_dev_major",
                "guest_dev_minor",
                "guest_inode",
                "guest_size",
                "guest_mappings",
                "translator_sha256",
            }
            or not isinstance(listener, dict)
            or guest["pid"] != owned["child_pid"]
            or guest["start_ticks"] != owned["child_start_ticks"]
            or guest["guest_path"] != owned["guest_path"]
            or guest["guest_sha256"] != owned["guest_sha256"]
            or guest["guest_sha256"] != receipt["native_binary_sha256"]
            or guest["translator_sha256"] != listener.get("exe_sha256")
            or guest["argv"] != owned["argv"]
            or any(
                guest[name] != owned[name]
                for name in ("guest_dev_major", "guest_dev_minor", "guest_inode", "guest_size")
            )
            or not isinstance(mappings, list)
            or not mappings
            or not any(
                isinstance(row, dict) and "x" in row.get("permissions", "") for row in mappings
            )
            or any(
                not isinstance(row, dict)
                or set(row) != {"permissions", "device", "inode", "path"}
                or row["path"] != owned["guest_path"]
                or row["inode"] != owned["guest_inode"]
                or row["device"] != f"{owned['guest_dev_major']:02x}:{owned['guest_dev_minor']:02x}"
                for row in mappings
            )
            or owned["argv"]
            != [
                owned["guest_path"],
                "-index",
                runtime["mount_destination"],
                "-listen",
                f"127.0.0.1:{listener['port']}",
                "-html=true",
            ]
            or listener["pid"] != owned["child_pid"]
            or listener["start_ticks"] != owned["child_start_ticks"]
        ):
            raise ValueError("native Zoekt guest mapping or translator proof differs")
        service_cleanup = document(root / "owned-service-cleanup.json")
        if service_cleanup != {
            "invocation_id": owned["invocation_id"],
            "tombstone_created": True,
            "owned_child_stopped": True,
            "owned_supervisor_stopped": True,
        }:
            raise ValueError("native Zoekt owned service cleanup differs")
        if raw(root / "native-translator").sha256 != "sha256:" + guest["translator_sha256"]:
            raise ValueError("native Zoekt translator executable bytes differ")
    elif "native_owned_service" in precommit or "native_owned_service" in result:
        raise ValueError("native Zoekt owned service receipt is missing")
    server_process = precommit.get("native_server_process")
    if server_process is not None or "native_server_process" in result:
        if (
            not isinstance(server_process, dict)
            or set(server_process) != {"port", "pid", "start_ticks", "exe_sha256"}
            or type(server_process["port"]) is not int
            or not 1 <= server_process["port"] <= 65535
            or type(server_process["pid"]) is not int
            or server_process["pid"] <= 0
            or type(server_process["start_ticks"]) is not int
            or server_process["start_ticks"] <= 0
            or server_process["exe_sha256"]
            != (
                receipt["native_binary_sha256"]
                if owned is None
                else owned["guest"]["translator_sha256"]
            )
            or result.get("native_server_process") != server_process
            or (owned is not None and server_process != owned["listener"])
        ):
            raise ValueError("native Zoekt listener process proof differs")
    controls = precommit.get("control_sha256")
    if controls is not None or "control_sha256" in result:
        if (
            not isinstance(controls, dict)
            or not controls
            or result.get("control_sha256") != controls
            or any(
                not isinstance(name, str)
                or re.fullmatch(r"[0-9a-f]{64}", value) is None
                or RawFile.capture(_absolute(name)).sha256 != "sha256:" + value
                for name, value in controls.items()
            )
        ):
            raise ValueError("native content audit control bytes differ")
    invocation = precommit.get("reader_invocation_id")
    if (
        invocation is not None
        or "reader_invocation_id" in result
        or "reader_invocation_id" in cleanup
    ):
        reader = result.get("reader_identity")
        remote = cleanup.get("remote_cleanup")
        if (
            not isinstance(invocation, str)
            or re.fullmatch(r"[0-9a-f]{32}", invocation) is None
            or result.get("reader_invocation_id") != invocation
            or cleanup.get("reader_invocation_id") != invocation
            or not isinstance(reader, dict)
            or set(reader) != {"kind", "invocation_id", "pid", "start_ticks"}
            or reader["kind"] != "native_worker_start"
            or reader["invocation_id"] != invocation
            or type(reader["pid"]) is not int
            or reader["pid"] <= 0
            or type(reader["start_ticks"]) is not int
            or reader["start_ticks"] <= 0
            or cleanup.get("reader_identity") != reader
            or not isinstance(remote, dict)
            or set(remote) != {"invocation_id", "matched", "stopped", "tombstone_created"}
            or remote["invocation_id"] != invocation
            or remote["stopped"] is not True
            or remote["tombstone_created"] is not True
            or not isinstance(remote["matched"], list)
            or len(remote["matched"]) > 1
            or any(
                not isinstance(match, dict)
                or match.get("pid") != reader["pid"]
                or match.get("start_ticks") != reader["start_ticks"]
                or match.get("state") not in ("R", "S", "D", "I", "T", "Z")
                for match in remote["matched"]
            )
        ):
            raise ValueError("native content audit owned reader cleanup differs")
    if (
        precommit["native_runtime"] != native
        or precommit["native_binary_sha256"] != receipt["native_binary_sha256"]
        or raw(root / "zoekt-webserver").sha256 != "sha256:" + receipt["native_binary_sha256"]
        or cleanup["deployed_binary_sha256"] != receipt["native_binary_sha256"]
        or cleanup["only_owned_reader_stopped"] is not True
        or raw(root / "native-worker.py").sha256 != "sha256:" + precommit["worker_sha256"]
        or raw(root / "probe_native_contents.py").sha256 != "sha256:" + precommit["script_sha256"]
        or result["status"] != "VERIFIED"
        or result["qualified"] is not False
        or result["bindings_unchanged"] is not True
        or result["failures"] != []
        or type(result["worker_exit"]) is not int
        or result["worker_exit"] != 0
        or result["release_digest"] != release_digest
        or result["rows_sha256"] != receipt["native_rows_sha256"]
    ):
        raise ValueError("native content audit custody differs")
    rows = raw(root / "native-rows.jsonl")
    if rows.sha256 != "sha256:" + receipt["native_rows_sha256"]:
        raise ValueError("native content audit row digest differs")
    expected = {row["path"]: row["file_sha256"] for row in files}
    seen: set[tuple[str, str]] = set()
    selected: set[str] = set()

    def consume(lines):
        for line in lines:
            row = parse_json(line.decode("utf-8"))
            if not isinstance(row, dict) or set(row) != {
                "repository",
                "path",
                "file_sha256",
                "actual_sha256",
                "http_status",
                "bytes",
                "matches",
                "seconds",
            }:
                raise ValueError("native content audit row shape differs")
            sourcegraph._path(row["path"])
            if not isinstance(row["repository"], str):
                raise ValueError("native content audit repository differs")
            key = (row["repository"], row["path"])
            if key in seen:
                raise ValueError("native content audit duplicate row")
            seen.add(key)
            if (
                type(row["http_status"]) is not int
                or row["http_status"] != 200
                or row["matches"] is not True
                or type(row["bytes"]) is not int
                or row["bytes"] < 0
                or type(row["seconds"]) not in (int, float)
                or not math.isfinite(row["seconds"])
                or row["seconds"] < 0
            ):
                raise ValueError("native content audit unsuccessful row")
            if row["repository"] == repo:
                if expected.get(row["path"]) != row["file_sha256"]:
                    raise ValueError("native content audit path or manifest digest differs")
                body = raw(root / "native-file-bodies" / repo / row["path"])
                if (
                    body.sha256 != "sha256:" + row["actual_sha256"]
                    or row["actual_sha256"] != expected[row["path"]]
                    or body.size != row["bytes"]
                ):
                    raise ValueError("native content audit stored bytes differ")
                selected.add(row["path"])

    rows.consume_lines(consume)
    if (
        selected != set(expected)
        or type(precommit["tasks"]) is not int
        or len(seen) != precommit["tasks"]
        or any(
            type(result[k]) is not int or result[k] != len(seen)
            for k in ("expected_files", "observed_files", "matched_files")
        )
    ):
        raise ValueError("native content audit incomplete path coverage")
    manifest_inputs = [
        path for path in precommit["manifest_sha256"] if _absolute(path).parent.name == repo
    ]
    if len(manifest_inputs) != 1:
        raise ValueError("native content audit manifest input is absent or duplicated")
    manifest_path = _absolute(manifest_inputs[0])
    if read(manifest_path) != manifest_raw or precommit["manifest_sha256"][
        str(manifest_path)
    ] != sourcegraph.sha256(manifest_raw):
        raise ValueError("native content audit manifest input differs")
    paths_path = _absolute(receipt["path_inventory_proof"])
    paths = document(paths_path)
    path_before = document(paths_path.parent / "native-index-before.json")
    path_after = document(paths_path.parent / "native-index-after.json")
    if (
        paths["qualified"] is not False
        or paths["status"] != "path_index_observed"
        or paths["release_digest"] != release_digest
        or paths["server_image_digest"] != config["server_image_digest"]
        or path_before != snapshot
        or path_after != snapshot
        or paths["native_index_sha256"] != snapshot["tree_sha256"]
    ):
        raise ValueError("indexed path audit differs from query capture")
    selected_rows = [row for row in paths["repositories"] if row["repository"] == repo]
    if len(selected_rows) != 1:
        raise ValueError("indexed path audit repository is absent or duplicated")
    path_row = selected_rows[0]
    revision = projection["projection_revision"]
    stream = read(paths_path.parent / f"{repo}.stream", MAX_STREAM_BYTES)
    if (
        path_row["raw_stream_sha256"] != sourcegraph.sha256(stream)
        or path_row["source_commit"] != manifest["repository_commit"]
        or path_row["projection_commit"] != revision
        or type(path_row["files"]) is not int
        or path_row["files"] != len(files)
        or path_row["query"]
        != f".* repo:^benchmark/{repo}$ rev:{revision} type:path patternType:regexp count:all"
    ):
        raise ValueError("indexed path audit request identity differs")
    if (
        _path_stream_inventory(
            stream,
            repository=config["repository"],
            revision=revision,
            expected=set(expected),
        )
        != path_row["native_match_count"]
    ):
        raise ValueError("indexed path audit incomplete or foreign inventory")
    for item in commitments.values():
        if RawFile.capture(item.path) != item:
            raise ValueError("index scope evidence changed during replay")
    return {
        "scope": SCOPE,
        "proof_level": (
            "owned_guest_translator_v1" if owned is not None else "legacy_native_scope_v1"
        ),
        "files": len(files),
        "receipt_sha256": commitments[str(receipt_path.absolute())].sha256,
        "evidence_sha256": "sha256:"
        + sourcegraph.sha256(
            canonical_json(
                {
                    path: {"sha256": item.sha256, "bytes": item.size}
                    for path, item in sorted(commitments.items())
                }
            ).encode()
        ),
        "backend_tree_sha256": snapshot["tree_sha256"],
        "projection_revision": revision,
        "native_binary_sha256": receipt["native_binary_sha256"],
    }


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description="Capture bounded Sourcegraph native index scope")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--live-spec", type=Path)
    source.add_argument("--scope-spec", type=Path)
    source.add_argument("--scope-batch", type=Path)
    parser.add_argument("--output-root", type=Path)
    parser.add_argument("--native-port", type=int, required=True)
    parser.add_argument("--native-binary-path")
    args = parser.parse_args()
    try:
        if args.scope_batch is not None:
            if args.output_root is not None:
                raise ValueError("index scope batch defines output roots per cell")
            receipts = capture_scope_batch(
                args.scope_batch,
                native_port=args.native_port,
                native_binary_path=args.native_binary_path,
            )
        else:
            if args.output_root is None:
                raise ValueError("index scope capture requires --output-root")
            receipts = [
                capture_from_live_spec(
                    args.live_spec if args.live_spec is not None else args.scope_spec,
                    args.output_root,
                    native_port=args.native_port,
                    native_binary_path=args.native_binary_path,
                    scope_spec=args.scope_spec is not None,
                )
            ]
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(2, f"ERROR: {error}\n")
    for receipt in receipts:
        print(receipt, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
