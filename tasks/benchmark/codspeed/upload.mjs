/*
 * Combine benchmark data from different jobs, and upload to Codspeed.
 */

import {createReadStream} from 'fs';
import fs from 'fs/promises';
import {join as pathJoin, dirname} from 'path';
import {fileURLToPath} from 'url';
import {createHash} from 'crypto';
import assert from 'assert';
import tar from 'tar';
import axios from 'axios';

const __dirname = dirname(fileURLToPath(import.meta.url));

const METADATA_SUFFIX = '_metadata.json',
    CODSPEED_UPLOAD_URL = 'https://api.codspeed.io/upload';

const dataDir = process.env.DATA_DIR,
    token = process.env.CODSPEED_TOKEN;

// Find profile files and first metadata file
const profiles = new Map(),
    components = new Set();
let metadataPath;
for (const filename of await fs.readdir(dataDir)) {
    const path = pathJoin(dataDir, filename);
    if (filename.endsWith(METADATA_SUFFIX)) {
        if (!metadataPath) metadataPath = path;
    } else {
        let match = filename.match(/^(.+)_(\d+)\.(out)$/);
        if (!match) {
            match = filename.match(/^(.+)_perf-(\d+)\.(map)$/);
            assert(match, `Unexpected file: ${filename}`);
        }
        
        const [, component, pid, type] = match,
            key = `${component}_${pid}`;
        let profile = profiles.get(key);
        if (!profile) {
            profile = {pid: +pid, outPath: null, mapPath: null};
            profiles.set(key, profile);
        }
        profile[`${type}Path`] = path;
        
        components.add(component);
    }
}

// Add cached results for benchmarks which weren't run
const cacheZipPath = pathJoin(__dirname, 'cachedBenches.tar.gz'),
    cacheDir = pathJoin(dataDir, 'cache');
await fs.mkdir(cacheDir);
await tar.extract({file: cacheZipPath, cwd: cacheDir});

for (const filename of await fs.readdir(cacheDir)) {
    const [, component, pid] = filename.match(/^(.+)_(\d+)\.out$/);
    if (components.has(component)) continue;
    
    const outPath = pathJoin(dataDir, filename);
    await fs.rename(pathJoin(cacheDir, filename), outPath);
    profiles.set(`${component}_${pid}`, {pid: +pid, outPath, mapPath: null});
}

// Move all profile files to one directory
console.log('Combining profiles');

const outDir = pathJoin(dataDir, 'out');
await fs.mkdir(outDir);

const pids = new Set(),
    duplicates = [];
let highestPid = -1;
for (const {pid, outPath, mapPath} of profiles.values()) {
    assert(outPath, `map file with no accompanying out file: ${mapPath}`);

    if (pids.has(pid)) {
        // Duplicate PID
        duplicates.push({pid, outPath, mapPath});
    } else {
        pids.add(pid);
        if (pid > highestPid) highestPid = pid;
        await fs.rename(outPath, pathJoin(outDir, `${pid}.out`));
        if (mapPath) await fs.rename(mapPath, pathJoin(outDir, `perf-${pid}.map`));
    }
}

// Alter PIDs for `.out` files with duplicate filenames, and rename accompanying `.map` files
for (let {pid, outPath, mapPath} of duplicates) {
    let content = await fs.readFile(outPath, 'utf8');

    const pidLine = `\npid: ${pid}\n`;
    const index = content.indexOf(pidLine);
    assert(index !== -1, `Could not locate PID in ${outPath}`);
    const before = content.slice(0, index);
    assert(before.split('\n').length === 3, `Unexpected formatting in ${outPath}`);

    pid = ++highestPid;
    content = `${before}\npid: ${pid}\n${content.slice(index + pidLine.length)}`;

    await fs.writeFile(pathJoin(outDir, `${pid}.out`), content);
    await fs.rm(outPath);
    if (mapPath) await fs.rename(mapPath, pathJoin(outDir, `perf-${pid}.map`));
}

// ZIP combined profile directory
console.log('Zipping combined profile directory');
const archivePath = pathJoin(dataDir, 'archive.tar.gz');
await tar.create({file: archivePath, gzip: true, cwd: outDir}, ['./']);

// Get size + MD5 hash of archive
console.log('Hashing ZIP');
const {size} = await fs.stat(archivePath);

const hash = createHash('md5');
const inputStream = createReadStream(archivePath);
for await (const chunk of inputStream) {
    hash.update(chunk);
}
const md5 = hash.digest('base64');

// Alter MD5 hash in metadata object
const metadata = JSON.parse(await fs.readFile(metadataPath, 'utf8'));
metadata.profileMd5 = md5;

// If no token, set `metadata.tokenless`, and log hash of metadata JSON.
// For tokenless runs (PRs from forks), `codspeed-runner` logs SHA256 hash of metadata JSON.
// CodSpeed then reads the job logs to find a line matching `CodSpeed Run Hash: "..."`.
// So we used a dummy token for `CodSpeedHQ/action` to prevent it logging the hash,
// so can log the correct hash ourselves here instead.
if (!token) metadata.tokenless = true;
const metadataJson = JSON.stringify(metadata);
if (!token) {
    const metadataHash = createHash('sha256').update(metadataJson).digest('hex');
    console.log(`CodSpeed Run Hash: "${metadataHash}"`);
}

// Upload metadata to CodSpeed
console.log('Uploading metadata to CodSpeed');
const {data} = await axios({
    method: 'post',
    url: CODSPEED_UPLOAD_URL,
    data: metadataJson,
    headers: {
        'Content-Type': 'application/json',
        ...(token ? {Authorization: token} : null),
    },
});
assert(data?.status === 'success', 'Failed to upload metadata to Codspeed');
const {uploadUrl} = data;

// Upload profile ZIP to Codspeed
console.log('Uploading profile ZIP to CodSpeed');
await axios({
    method: 'put',
    url: uploadUrl,
    data: createReadStream(archivePath),
    headers: {
        'Content-Type': 'application/gzip',
        'Content-Length': size,
        'Content-MD5': md5,
    }
});
