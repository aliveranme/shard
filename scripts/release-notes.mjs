import { readFileSync } from 'node:fs';

const number = '(?:0|[1-9][0-9]*)';
const prerelease = `(?:${number}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)`;
const build = '[0-9A-Za-z-]+';
const tagPattern = new RegExp(
  `^v(${number}\\.${number}\\.${number}(?:-${prerelease}(?:\\.${prerelease})*)?(?:\\+${build}(?:\\.${build})*)?)(?![\\s\\S])`,
);
const version = tagPattern.exec(process.argv[2])?.[1];
if (!version) {
  console.error('Use a v-prefixed SemVer tag such as v0.2.0 or v0.2.0-rc.1.');
  process.exit(1);
}

const changelog = readFileSync(new URL('../CHANGELOG.md', import.meta.url), 'utf8');
let fence;
let activeSection;
const matches = [];
for (const line of changelog.split(/\r?\n/)) {
  if (fence) {
    const closing = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line)?.[1];
    if (closing?.[0] === fence[0] && closing.length >= fence.length) fence = undefined;
  } else {
    fence = /^ {0,3}(`{3,})(?:[^`]*)$/.exec(line)?.[1] ?? /^ {0,3}(~{3,})/.exec(line)?.[1];
    const heading = /^ {0,3}(#{1,2})[ \t]+(.*?)[ \t]*$/.exec(line);
    if (heading) {
      activeSection = undefined;
      if (heading[1] === '##' && heading[2] === version) {
        activeSection = [];
        matches.push(activeSection);
      }
      continue;
    }
  }
  activeSection?.push(line);
}

const notes = matches[0]?.join('\n').replace(/^(?:[ \t]*\n)+|\n(?:[ \t]*\n)*[ \t]*$/g, '');
if (matches.length !== 1 || !notes?.trim()) {
  console.error(`Add exactly one nonempty "## ${version}" section to CHANGELOG.md in the tagged commit.`);
  process.exit(1);
}
process.stdout.write(`${notes}\n`);
