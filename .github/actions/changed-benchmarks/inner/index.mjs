import {readFile} from 'fs/promises';
import {join as pathJoin, sep as pathSep} from 'path';
import {fileURLToPath} from 'url';
import assert from 'assert';
import * as core from '@actions/core';
import {parse as parseToml} from '@iarna/toml';

const BENCHMARKS_CARGO_TOML_FILENAME = 'tasks/benchmark/Cargo.toml';

let DIR_PATH = pathJoin(fileURLToPath(import.meta.url), '..');
const IS_DEV = !DIR_PATH.endsWith(`${pathSep}dist`);
if (!IS_DEV) DIR_PATH = DIR_PATH.slice(0, -pathSep.length - 4);
const ROOT_DIR_PATH = pathJoin(DIR_PATH, '../../../..'),
    CRATES_DIR_PATH = pathJoin(ROOT_DIR_PATH, 'crates');

if (IS_DEV) {
    // Local testing
    const CHANGED_DIRS = ['crates/oxc_semantic'];
    const BENCHMARKS = [
        'codegen_sourcemap', 'lexer', 'minifier', 'parser', 'semantic', 'sourcemap', 'transformer',
    ];
    const SEPARATE_BENCHMARKS = ['linter'];

    console.log('All benchmarks:', [...BENCHMARKS, ...SEPARATE_BENCHMARKS]);
    await getOutputs(CHANGED_DIRS, BENCHMARKS, SEPARATE_BENCHMARKS);
} else {
    // On CI
    run();
}

async function run() {
    try {
        // Parse inputs
        const changedDirs = JSON.parse(core.getInput('changed_dirs')),
            benchmarks = parseInputList(core.getInput('benchmarks')),
            separateBenchmarks = parseInputList(core.getInput('separate_benchmarks'));

        // Get which benchmarks need to run, and output
        const output = await getOutputs(changedDirs, benchmarks, separateBenchmarks);
        core.setOutput('benchmarks', JSON.stringify(output.benchmarks));
        core.setOutput('separate_benchmarks', JSON.stringify(output.separateBenchmarks));
        core.setOutput('skip_benchmarks', JSON.stringify(output.skipBenchmarks));
        console.log('Done');
    } catch (err) {
        core.setFailed(err?.stack || 'Unknown error');
        console.log('FAILED');
    }
}

/*
 * Get which benchmarks need to run, and split into main/separate groups.
 */
async function getOutputs(changedDirs, mainBenchmarks, separateBenchmarks) {
    const allBenchmarks = [...mainBenchmarks, ...separateBenchmarks];
    const allBenchmarksToRun = new Set(await getBenchesToRun(changedDirs, allBenchmarks));

    const output = {benchmarks: null, separateBenchmarks: null, skipBenchmarks: []};
    const filter = benches => benches.filter((benchName) => {
        if (allBenchmarksToRun.has(benchName)) return true;
        output.skipBenchmarks.push(benchName);
        return false;
    });
    output.benchmarks = filter(mainBenchmarks);
    output.separateBenchmarks = filter(separateBenchmarks);

    console.log('Run benchmarks:', output.benchmarks);
    console.log('Run benchmarks separately:', output.separateBenchmarks);
    console.log('Skip benchmarks:', output.skipBenchmarks);

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
async function getBenchesToRun(changedDirs, benchmarks) {
    // Get crates which have changed
    const changedCrates = new Set();
    for (const dir of changedDirs) {
        // If files outside `crates` dir changed, run all benchmarks
        if (!dir.startsWith('crates/')) return benchmarks;
        const crate = dir.slice('crates/'.length);
        changedCrates.add(crate);
    }

    // Get which `oxc_*` crates benchmarks depend on
    const benchesCargoToml = parseToml(
        await readFile(pathJoin(ROOT_DIR_PATH, BENCHMARKS_CARGO_TOML_FILENAME), 'utf8')
    );

    const benchmarkDependencies = new Map();
    for (const benchName of benchmarks) {
        let deps = benchesCargoToml.features?.[benchName];
        assert(deps, `No feature in '${BENCHMARKS_CARGO_TOML_FILENAME}' for benchmark '${benchName}`);
        benchmarkDependencies.set(
            benchName,
            new Set(deps.flatMap((dep) => {
                if (dep.startsWith('dep:')) dep = dep.slice(4);
                return isOxcCrate(dep) ? [dep] : [];
            }))
        );
    }

    // Extend list of dependencies for each benchmark to include transitive dependencies
    await getAllDependencies(benchmarkDependencies);

    // Find benchmarks which need to re-run
    return benchmarks.filter(
        benchName => [...benchmarkDependencies.get(benchName)].some(dep => changedCrates.has(dep))
    );
}

/*
 * Get all dependencies of benchmarks on `oxc_*` crates (including transitive dependencies).
 */
async function getAllDependencies(benchDepsMap) {
    // Initialize map of crates we need to find all dependencies of
    const depsMap = new Map();
    for (const benchDeps of benchDepsMap.values()) {
        for (const dep of benchDeps) {
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

    // Add recursive dependencies to benchmark dependencies map
    for (const benchDeps of benchDepsMap.values()) {
        for (const benchDep of benchDeps) {
            for (const transitiveDep of depsMap.get(benchDep)) {
                benchDeps.add(transitiveDep);
            }
        }
    }
}

/*
 * Get direct dependencies of a crate.
 */
async function getCrateDependencies(crateName) {
    let cargoToml;
    try {
        cargoToml = await readFile(pathJoin(CRATES_DIR_PATH, crateName, 'Cargo.toml'), 'utf8');
    } catch (err) {
        if (err?.code === 'ENOENT') return [];
        throw err;
    }
    
    const {dependencies} = parseToml(cargoToml);
    if (!dependencies) return [];
    return Object.keys(dependencies).filter(isOxcCrate);
}

function parseInputList(list) {
    return list.split('\n').map(line => line.trim()).filter(line => line && !line.startsWith('#'));
}

function isOxcCrate(crate) {
    return crate === 'oxc' || crate.startsWith('oxc_');
}
