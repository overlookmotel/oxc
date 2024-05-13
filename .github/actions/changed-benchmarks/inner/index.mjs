import fs from 'fs/promises';
import {join as pathJoin, sep as pathSep} from 'path';
import {fileURLToPath} from 'url';
import assert from 'assert';
import * as core from '@actions/core';
import * as github from '@actions/github';
import artifact from '@actions/artifact';
import {parse as parseToml} from '@iarna/toml';

const artifactClient = artifact.default;

const BENCHMARKS_CARGO_TOML_FILENAME = 'tasks/benchmark/Cargo.toml',
    REPO_OWNER = process.env.GITHUB_REPOSITORY_OWNER,
    REPO_NAME = process.env.GITHUB_REPOSITORY.slice(REPO_OWNER.length + 1),
    BENCHMARK_WORKFLOW_FILENAME = 'benchmark.yml',
    ARTEFACT_NAME_PREFIX = 'result-',
    METADATA_FILENAME_SUFFIX = '_metadata.json';

let DIR_PATH = pathJoin(fileURLToPath(import.meta.url), '..');
const IS_DEV = !DIR_PATH.endsWith(`${pathSep}dist`);
if (!IS_DEV) DIR_PATH = DIR_PATH.slice(0, -pathSep.length - 4);
const ROOT_DIR_PATH = pathJoin(DIR_PATH, '../../../..'),
    CRATES_DIR_PATH = pathJoin(ROOT_DIR_PATH, 'crates');

if (IS_DEV) {
    runDev();
} else {
    // On CI
    run();
}

async function run() {
    try {
        // Parse inputs
        const changedDirs = JSON.parse(core.getInput('changed_dirs')),
            benchmarks = parseInputList(core.getInput('benchmarks')),
            separateBenchmarks = parseInputList(core.getInput('separate_benchmarks')),
            baseSha = core.getInput('base_sha'),
            token = core.getInput('token'),
            retentionDays = +core.getInput('retention_days');

        // Get which benchmarks need to run, and output
        const output = await getOutputs(
            changedDirs, benchmarks, separateBenchmarks, baseSha, token, retentionDays
        );
        core.setOutput('benchmarks', JSON.stringify(output.benchmarks));
        core.setOutput('separate_benchmarks', JSON.stringify(output.separateBenchmarks));
        core.setOutput('workflow_run_id', output.workflowRunId || '');
        console.log('Done');
    } catch (err) {
        core.setFailed(err?.stack || 'Unknown error');
        console.log('FAILED');
    }
}

// Local testing
async function runDev() {
    const CHANGED_DIRS = ['crates/oxc_traverse'];
    const BENCHMARKS = [
        'codegen_sourcemap', 'lexer', 'minifier', 'parser', 'semantic', 'sourcemap', 'transformer',
    ];
    const SEPARATE_BENCHMARKS = ['linter x2'];
    const {TOKEN} = process.env;
    const BASE_SHA = 'e68fb6162e8d4864e84bf2ae092b53188cf7f44d';
    const RETENTION_DAYS = 1;

    console.log('All benchmarks:', [...BENCHMARKS, ...SEPARATE_BENCHMARKS]);
    await getOutputs(CHANGED_DIRS, BENCHMARKS, SEPARATE_BENCHMARKS, BASE_SHA, TOKEN, RETENTION_DAYS);
}

/*
 * Get which benchmarks need to run, and split into main/separate groups.
 */
async function getOutputs(
    changedDirs, mainBenchmarks, separateBenchmarks, baseSha, token, retentionDays
) {
    const allBenchmarks = new Map();
    function addBenchmarks(benchNames, isSeparate) {
        for (const benchNameAndFixturesCount of benchNames) {
            let [, benchName, numFixtures] = benchNameAndFixturesCount.match(/^(.+?)(?:\s+x(\d+))?$/);
            numFixtures = +(numFixtures || 1);

            allBenchmarks.set(benchName, {
                name: benchName,
                isSeparate,
                isSkipped: false,
                dependencies: null,
                artefacts: [],
                numFixtures,
            });
        }
    }
    addBenchmarks(mainBenchmarks, false);
    addBenchmarks(separateBenchmarks, true);

    const workflowRunId = await getBenchesToRun(
        changedDirs, allBenchmarks, baseSha, token, retentionDays
    );

    const output = {benchmarks: [], separateBenchmarks: [], workflowRunId},
        skipBenchmarks = [];
    for (const {name, isSkipped, isSeparate} of allBenchmarks.values()) {
        if (isSkipped) {
            skipBenchmarks.push(name);
        } else if (isSeparate) {
            output.separateBenchmarks.push(name);
        } else {
            output.benchmarks.push(name);
        }
    }

    function printList(title, benchmarkNames) {
        if (benchmarkNames.length === 0) {
            console.log(`${title}: (none)`);
        } else {
            console.log(`${title}:\n- ${benchmarkNames.join('\n- ')}`);
        }
    }

    printList('Run benchmarks', output.benchmarks);
    printList('Run benchmarks separately', output.separateBenchmarks);
    printList('Skip benchmarks', skipBenchmarks);
    console.log('Workflow run ID for previous benchmark run on main branch:', workflowRunId);
    console.log('Base SHA:', baseSha);

    return output;
}

/*
 * Given array of directories which have changes within them, get which benchmarks need to run,
 * benchmark may be affected by these changes.
 *
 * A change in an `oxc_*` crate means all benchmarks which depend on that crate
 * (including transitively) need to run.
 * Any change to files outside `crates` means *all* benchmarks must run.
 * The latter is conservative - it'd be too complicated and error-prone
 * to try to track other dependencies.
 */
async function getBenchesToRun(changedDirs, benchmarks, baseSha, token, retentionDays) {
    // Get crates which have changed
    const changedCrates = new Set();
    for (const dir of changedDirs) {
        // If files outside `crates` dir changed, run all benchmarks
        if (!dir.startsWith('crates/')) {
            if (retentionDays > 1) return await getWorkflowRunId(baseSha, token);
            return null;
        }
        const crate = dir.slice('crates/'.length);
        changedCrates.add(crate);
    }

    // Get which `oxc_*` crates benchmarks depend on
    const benchesCargoToml = parseToml(
        await fs.readFile(pathJoin(ROOT_DIR_PATH, BENCHMARKS_CARGO_TOML_FILENAME), 'utf8')
    );

    for (const benchmark of benchmarks.values()) {
        const dependencyNames = benchesCargoToml.features?.[benchmark.name];
        assert(
            dependencyNames,
            `No feature in '${BENCHMARKS_CARGO_TOML_FILENAME}' for benchmark '${benchmark.name}`
        );
        benchmark.dependencies = new Set(dependencyNames.flatMap((dep) => {
            if (dep.startsWith('dep:')) dep = dep.slice(4);
            return isOxcCrate(dep) ? [dep] : [];
        }));
    }

    // Extend list of dependencies for each benchmark to include transitive dependencies
    await getAllDependencies(benchmarks);

    // Find benchmarks which can be skipped
    for (const benchmark of benchmarks.values()) {
        if (![...benchmark.dependencies].some(dep => changedCrates.has(dep))) benchmark.isSkipped = true;
    }

    // Get artefacts.
    // Move from skip list to run list where artefacts are not available.
    const workflowRunId = await getResultArtefacts(benchmarks, baseSha, token, retentionDays);
    return workflowRunId;
}

/*
 * Get all dependencies of benchmarks on `oxc_*` crates (including transitive dependencies).
 */
async function getAllDependencies(benchmarks) {
    // Initialize map of crates we need to find all dependencies of
    const depsMap = new Map();
    for (const benchmark of benchmarks.values()) {
        for (const dep of benchmark.dependencies) {
            depsMap.set(dep, new Set());
        }
    }

    // Recursively get all `oxc_*` crates which those crates depend on
    let batchDeps = new Set(depsMap.keys());
    while (true) {
        const newDeps = new Set();
        for (const dep of batchDeps) {
            const crateDeps = await getCrateDependencies(dep);
            for (const crateDep of crateDeps) {
                depsMap.get(dep).add(crateDep);
                if (!depsMap.has(crateDep)) {
                    depsMap.set(crateDep, new Set());
                    newDeps.add(crateDep);
                }
            }
        }
        if (newDeps.size === 0) break;
        batchDeps = newDeps;
    }

    // Add recursive dependencies to benchmark dependencies
    for (const {dependencies} of benchmarks.values()) {
        for (const benchDep of dependencies) {
            for (const transitiveDep of depsMap.get(benchDep)) {
                dependencies.add(transitiveDep);
            }
        }
    }
}

/*
 * Get direct dependencies of a crate.
 */
async function getCrateDependencies(crateName) {
    // Get crate's `Cargo.toml`.
    // Ignore crates which aren't in `crates` dir. If changes were made to any files
    // outside `crates` dir, all benchmarks run.
    let cargoToml;
    try {
        cargoToml = await fs.readFile(pathJoin(CRATES_DIR_PATH, crateName, 'Cargo.toml'), 'utf8');
    } catch (err) {
        if (err?.code === 'ENOENT') return [];
        throw err;
    }

    const {dependencies} = parseToml(cargoToml);
    if (!dependencies) return [];
    return Object.keys(dependencies).filter(isOxcCrate);
}

/*
 * Get benchmark result artefacts from previous run on main branch.
 * If artefacts cannot be found for any of `skipBenchmarks`, they're added back into `runRenchmarks`.
 */
async function getResultArtefacts(benchmarks, baseSha, token, retentionDays) {
    let workflowRunId = null;
    const exitWithNoSkip = () => {
        for (const benchmark of benchmarks.values()) {
            benchmark.isSkipped = false;
        }
        return workflowRunId;
    };

    // Get workflow run ID for previous run on main
    try {
        workflowRunId = await getWorkflowRunId(baseSha, token);
        if (!workflowRunId) return exitWithNoSkip();
    } catch (err) {
        console.warn(`Failed to fetch artefacts: Could not get workflow run.\n${err.stack}`);
        return exitWithNoSkip();
    }

    // Exit if no benchmarks to skip
    let numSkipped = [...benchmarks.values()].filter(benchmark => benchmark.isSkipped).length;
    if (numSkipped === 0) return exitWithNoSkip();

    // Get list of artefacts for workflow run
    let artefacts;
    try {
        artefacts = await getArtefactsForWorkflow(workflowRunId, token);
    } catch (err) {
        console.warn(`Failed to fetch artefacts: Could not get artefacts list.\n${err.stack}`);
        return exitWithNoSkip();
    }

    // Organise artefacts by benchmark
    for (const artefact of artefacts) {
        if (!artefact.name.startsWith(ARTEFACT_NAME_PREFIX)) continue;
        const benchName = artefact.name.slice(ARTEFACT_NAME_PREFIX.length).match(/^(.+?)\d*$/)[1];
        const benchmark = benchmarks.get(benchName);
        if (!benchmark) continue;
        if (benchmark.isSkipped) benchmark.artefacts.push(artefact);
    }

    // Don't skip benchmarks where artefacts missing
    for (const benchmark of benchmarks.values()) {
        if (benchmark.isSkipped && benchmark.artefacts.length !== benchmark.numFixtures) {
            benchmark.isSkipped = false;
            numSkipped--;
        }
    }
    if (numSkipped === 0) return exitWithNoSkip();

    // Download artefacts
    const tempDirPath = await createTempDir();
    await Promise.all(
        [...benchmarks.values()]
        .filter(benchmark => benchmark.isSkipped)
        .map(async (benchmark) => {
            try {
                await downloadAndReuploadArtefacts(benchmark.artefacts, token, retentionDays, tempDirPath);
            } catch (err) {
                console.warn(`Failed to download/re-upload artefacts for ${benchmark.name}`);
                benchmark.isSkipped = false;
            }
        })
    );

    return workflowRunId;
}

async function createTempDir() {
    const path = `/tmp/oxc_bench_data_${Math.round(Math.random() * 1000000000000000000).toString(16)}`;
    await fs.mkdir(path);
    return path;
}

async function getWorkflowRunId(baseSha, token) {
    if (!baseSha || !token) {
        console.warn('Cannot get workflow run ID: No token or commit SHA provided');
        return null;
    }

    const octokit = github.getOctokit(token);
    const res = await octokit.rest.actions.listWorkflowRuns({
        owner: REPO_OWNER,
        repo: REPO_NAME,
        workflow_id: BENCHMARK_WORKFLOW_FILENAME,
        status: 'success',
        head_sha: baseSha,
    });
    assert(res.status === 200, 'Failed to list runs');
    return res.data.workflow_runs[0].id;
}

async function getArtefactsForWorkflow(workflowRunId, token) {
    const res = await artifactClient.listArtifacts({
        findBy: {
            token,
            workflowRunId,
            repositoryOwner: REPO_OWNER,
            repositoryName: REPO_NAME,
        }
    });
    return res.artifacts;
}

async function downloadAndReuploadArtefacts(artefacts, token, retentionDays, tempDirPath) {
    // Download artefacts and delete `*_metadata.json` file
    await Promise.all(artefacts.map(async (artefact) => {
        const dirPath = pathJoin(tempDirPath, artefact.name);
        console.log('Downloading artefact:', dirPath);
        await artifactClient.downloadArtifact(artefact.id, {
            path: dirPath,
            findBy: {
                token,
                repositoryOwner: REPO_OWNER,
                repositoryName: REPO_NAME,
            }
        });

        const filenames = await fs.readdir(dirPath);
        const filePaths = filenames
            .filter(filename => !filename.endsWith(METADATA_FILENAME_SUFFIX))
            .map(filename => pathJoin(dirPath, filename));

        // Re-upload artefact in current workflow
        if (IS_DEV) return;
        await artifactClient.uploadArtifact(artefact.name, filePaths, dirPath, {retentionDays});
    }));
}

function parseInputList(list) {
    return list.split('\n').map(line => line.trim()).filter(line => line && !line.startsWith('#'));
}

function isOxcCrate(crate) {
    return crate === 'oxc' || crate.startsWith('oxc_');
}
