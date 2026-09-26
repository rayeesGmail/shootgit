import type { RepoInfo, StatusEntry } from '@shootgit/ipc-types';
import { describe, expect, it } from 'vitest';

import { describeHead, describeUpstream, statusCode } from './status';

function entry(overrides: Partial<StatusEntry>): StatusEntry {
  return {
    path: 'file.txt',
    old_path: null,
    index_status: 'unmodified',
    worktree_status: 'modified',
    is_conflicted: false,
    is_submodule: false,
    ...overrides,
  };
}

function info(overrides: Partial<RepoInfo>): RepoInfo {
  return {
    id: 1,
    path: '/work/repo',
    head: { kind: 'branch', name: 'main', oid: '0123456789abcdef0123456789abcdef01234567' },
    upstream: null,
    ahead_behind: null,
    ...overrides,
  };
}

describe('statusCode', () => {
  it("spells the entry as git's two-letter XY code", () => {
    expect(statusCode(entry({}))).toBe('.M');
    expect(statusCode(entry({ index_status: 'added', worktree_status: 'unmodified' }))).toBe('A.');
    expect(statusCode(entry({ index_status: 'renamed', worktree_status: 'modified' }))).toBe('RM');
    expect(statusCode(entry({ index_status: 'copied', worktree_status: 'deleted' }))).toBe('CD');
    expect(statusCode(entry({ index_status: 'unmodified', worktree_status: 'type_changed' }))).toBe(
      '.T',
    );
    expect(
      statusCode(entry({ index_status: 'unmerged', worktree_status: 'unmerged', is_conflicted: true })),
    ).toBe('UU');
  });

  it('uses ?? and !! for untracked and ignored paths, as porcelain v1 does', () => {
    expect(statusCode(entry({ index_status: 'untracked', worktree_status: 'untracked' }))).toBe('??');
    expect(statusCode(entry({ index_status: 'ignored', worktree_status: 'ignored' }))).toBe('!!');
  });
});

describe('describeHead', () => {
  it('names the branch and its short commit id', () => {
    expect(describeHead(info({}))).toBe('main @ 0123456');
  });

  it('says when a branch has no commits yet', () => {
    expect(describeHead(info({ head: { kind: 'unborn', name: 'trunk' } }))).toBe(
      'trunk (no commits yet)',
    );
  });

  it('says when HEAD is detached', () => {
    expect(
      describeHead(
        info({ head: { kind: 'detached', oid: 'fedcba9876543210fedcba9876543210fedcba98' } }),
      ),
    ).toBe('detached at fedcba9');
  });
});

describe('describeUpstream', () => {
  it('is empty without an upstream', () => {
    expect(describeUpstream(info({}))).toBe('');
  });

  it('shows the upstream with how far HEAD is ahead and behind', () => {
    expect(
      describeUpstream(info({ upstream: 'origin/main', ahead_behind: { ahead: 2, behind: 0 } })),
    ).toBe('origin/main ↑2 ↓0');
  });

  it('shows an upstream whose branch is gone without counts', () => {
    expect(describeUpstream(info({ upstream: 'origin/gone' }))).toBe('origin/gone (gone)');
  });
});
