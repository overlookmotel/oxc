/*
 * Combine benchmark data from different jobs, and upload to Codspeed.
 */

import {createReadStream} from 'fs';
import fs from 'fs/promises';
import {join as pathJoin} from 'path';
import {createHash} from 'crypto';
import assert from 'assert';
import tar from 'tar';
import axios from 'axios';

const METADATA_SUFFIX = '_metadata.json',
    CODSPEED_UPLOAD_URL = 'https://api.codspeed.io/upload';

const dataDir = process.env.DATA_DIR,
    token = process.env.CODSPEED_TOKEN;

// Find profile files and first metadata file.
// Match `.out` and `.map` files to each other.
const profiles = new Map();
let metadataPath;
for (const filename of await fs.readdir(dataDir)) {
    const path = pathJoin(dataDir, filename);
    if (filename.endsWith(METADATA_SUFFIX)) {
        if (!metadataPath) metadataPath = path;
    } else {
        let type;
        let match = filename.match(/^(.*)_(\d+)\.out$/);
        if (match) {
            type = 'out';
        } else {
            match = filename.match(/^(.*)_perf-(\d+)\.map$/);
            assert(match, `Unexpected file: ${filename}`);
            type = 'map';
        }

        const component = match[1],
            pid = +match[2],
            key = `${component}_${pid}`;
        let profile = profiles.get(key);
        if (!profile) {
            profile = {pid, outPath: null, mapPath: null};
            profiles.set(key, profile);
        }
        profile[`${type}Path`] = path;
    }
}

// Move all profile files to one directory
console.log('Combining profiles');

const outDir = pathJoin(dataDir, 'out');
await fs.mkdir(outDir);

const pids = new Set(),
    duplicates = [];
let highestPid = -1;
for (const {pid, outPath, mapPath} of profiles.values()) {
    assert(outPath, `map file with no corresponding out file: ${mapPath}`);

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

// Alter PIDs for profile files with duplicate PIDs
for (let {pid, outPath, mapPath} of duplicates) {
    let outContent = await fs.readFile(outPath, 'utf8');

    const pidLine = `\npid: ${pid}\n`;
    const index = outContent.indexOf(pidLine);
    assert(index !== -1, `Could not locate PID in ${outPath}`);
    const before = outContent.slice(0, index);
    assert(before.split('\n').length === 3, `Unexpected formatting in ${outPath}`);

    pid = ++highestPid;
    outContent = `${before}\npid: ${pid}\n${outContent.slice(index + pidLine.length)}`;

    await fs.writeFile(pathJoin(outDir, `${pid}.out`), outContent);
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
