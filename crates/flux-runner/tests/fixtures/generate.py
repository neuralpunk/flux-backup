# Run inside: unshare -rUmu --fork, with hostname fixture-host and a work folder at /srv.
# Usage: gen_fixture.py <flux binary> <engines,comma,separated>
import hashlib, json, os, subprocess, sys, shutil, random
flux, engines = sys.argv[1], sys.argv[2].split(",")
M = "/srv"
env = dict(os.environ, FLUX_HOME=f"{M}/home", FLUX_PASSWORD="fixture", FLUX_SECRET_TOOL="false",
           FLUX_SYSTEMCTL="true", FLUX_SYSTEMD_UNIT_DIR=f"{M}/units", FLUX_CRONTAB=f"{M}/nocron",
           FLUX_CRON_DIR=f"{M}/cron.d", FLUX_SHUTDOWN="false", BORG_BASE_DIR=f"{M}/borg-base",
           HOME=f"{M}/userhome", USER="root")
def run(*a):
    r = subprocess.run([flux, *a], env=env, capture_output=True, text=True)
    if r.returncode != 0:
        print("FAILED", a, r.stdout, r.stderr, file=sys.stderr); sys.exit(1)
    return r.stdout
src = f"{M}/src"
os.makedirs(f"{src}/docs/deep", exist_ok=True)
os.makedirs(f"{M}/userhome", exist_ok=True)
os.makedirs(f"{M}/tgt", exist_ok=True)
open(f"{src}/docs/a.txt", "w").write("alpha\n")
open(f"{src}/docs/deep/ünï cödé.txt", "w").write("unicode\n")
open(f"{src}/docs/empty", "w").close()
rnd = random.Random(42)
open(f"{src}/data.bin", "wb").write(bytes(rnd.getrandbits(8) for _ in range(32 * 1024)))
os.symlink("docs/a.txt", f"{src}/link")
with open(f"{src}/sparse.img", "wb") as f:
    f.truncate(256 * 1024); f.seek(128 * 1024); f.write(b"in the middle")
os.chmod(f"{src}/docs/a.txt", 0o640)
for p in ["docs/a.txt", "docs/deep/ünï cödé.txt", "docs/empty", "data.bin", "sparse.img"]:
    os.utime(f"{src}/{p}", (1768478400, 1768478400))   # 2026-01-15 12:00 UTC
for e in engines:
    run("profile", "create", e, "-t", f"{M}/tgt", "-s", src, "--engine", e)
    path = f"{M}/home/config/profiles/{e}.toml"
    t = open(path).read().replace("notify = true", "notify = false")
    open(path, "w").write(t)
    run("backup", e)
def hashes():
    out = {}
    for root, dirs, files in os.walk(src):
        for n in files:
            p = os.path.join(root, n)
            rel = os.path.relpath(p, src)
            if os.path.islink(p):
                out[rel] = "link:" + os.readlink(p)
            else:
                out[rel] = hashlib.sha256(open(p, "rb").read()).hexdigest()
    return out
first = hashes()
open(f"{src}/docs/b.txt", "w").write("beta, added later\n")
open(f"{src}/docs/a.txt", "w").write("alpha, changed\n")
os.utime(f"{src}/docs/a.txt", (1768564800, 1768564800))
os.utime(f"{src}/docs/b.txt", (1768564800, 1768564800))
import time; time.sleep(1.1)   # snapshot ids are to the second
for e in engines:
    run("backup", e)
second = hashes()
version = run("--version").strip()
json.dump({"flux": version, "host": "fixture-host", "user": "root", "source": src,
           "password": "fixture", "engines": engines, "first": first, "latest": second},
          open(f"{M}/expected.json", "w"), indent=1, sort_keys=True, ensure_ascii=False)
print("generated", version, engines)
