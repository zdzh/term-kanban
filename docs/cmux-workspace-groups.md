# cmux 工作区分组：同仓库的任务落在同一个分组下

## 问题

cmux 里一次「打开工作区」会在侧边栏新加一条独立的 workspace。一个仓库干十件事就是十条，
彼此看不出关系。cmux 提供了 `workspace-group`（可折叠的侧边栏分组），但需要显式调用才用得上。

**目标**：同一个 git 仓库的所有看板工作区，自动归到以仓库名命名的 cmux 分组下面。

## cmux 的相关能力（实测自 cmux 0.6x）

| 命令 | 作用 |
|---|---|
| `cmux workspace-group list --json` | 列出分组，含 `name` / `ref` / `member_workspace_refs` / `anchor_workspace_is_generated` |
| `cmux workspace-group create --name <n> --idempotency-key <k>` | 建组；**同 key 并发/重试只建一个**，输出 `OK workspace_group:N` |
| `cmux workspace-group add --group <g> --workspace <w>` | 把已有 workspace 加进组 |
| `cmux workspace-group set-anchor --group <g> --workspace <w>` | 换锚点 |
| `cmux workspace create --cwd <p> --name <t> --group <g> --group-placement end` | 直接建在组里 |
| `cmux workspace list --json` | 每个 workspace 有 `current_directory`，可据此判断"这个 worktree 是否已经开着" |

两个容易踩的点：

1. **建组会自动生成一个锚点 workspace**（`anchor_workspace_is_generated: true`），它落在调用者的当前目录、名字就是组名。侧边栏上这条空白条目很突兀，需要在第一个真实成员进来后 `set-anchor` 并把旧的关掉。
2. **分组归属靠 `current_directory` 精确匹配**，不是靠路径前缀——所以 worktree 路径必须和 `cmux workspace create --cwd` 传的完全一致。

## 实现

`scripts/kanban-open-ws`（仓库内，可执行）：

```
kanban-open-ws --repo {repo} --worktree {worktree} --branch {branch} --title {title}
```

流程：

1. worktree 不存在 → `git worktree add`（分支已存在就复用分支，否则 `-b`）；存在则什么都不做
2. `group_ref()`：先按仓库目录名找同名组；没有就 `workspace group create --idempotency-key "repo:<仓库绝对路径>"`
3. `existing_ws()`：按 `current_directory == {worktree}` 找回已有 workspace
   - 找到 → 补一次 `group add`（防止它被手动挪出组）+ `workspace select`，退出
   - 没找到 → `workspace create --group <g> --group-placement end`
4. `adopt_anchor()`：若组锚点是自动生成的，换成刚建的真实 workspace，并关掉那个空锚点

## 接到看板里（两种方式）

### 方式一：看板自动补分组（推荐）

打开命令保持 `cmux {worktree}`，看板在调用前会自动：

1. 按 `idempotency_key = repo:<真实路径>` 找分组，找不到再按仓库目录名找
2. 都没有就 `workspace group create --name <仓库名> --idempotency-key repo:<真实路径>`
3. 把 `--group <ref>` 追加到命令末尾
4. 打开成功后 `adopt_cmux_anchor()`：若锚点是自动生成的，换成刚开的真实 workspace 并关掉空锚点

**只在命令以 `cmux` 开头、是"开目录"形式（`cmux <路径>` / `cmux open <路径>`）且不含 `--group` 时才动手。**
`code` / `codex` / 自定义脚本一律不干预。

### 方式二：用脚本（需要更复杂的流程时）

```
/Users/pikaka/work/task-kanban/scripts/kanban-open-ws \
  --repo {repo} --worktree {worktree} --branch {branch} --title {title_arg}
```

脚本额外做的事：复用已有 workspace（按 `current_directory` 精确匹配）、给 workspace 设置标题、`--group-placement end`。

> 注意用 `{title_arg}` 而不是 `{title}`：前者会被转义成**一个参数**，标题里有空格、
> 分号也不会被 shell 解释；后者仍然禁止出现在命令里。

想换 codex / vscode 仍然只是改这一行里的命令，脚本本身与工具无关。

## 验收（已实测）

| 场景 | 结果 |
|---|---|
| 首次打开某个 worktree | 建 worktree + 建组 + 建 workspace 并归组，锚点换成真实成员，空锚点被关闭 |
| 同一仓库打开第二张卡 | 复用同一个组（成员数 2，仍是 1 个组） |
| 重复点同一张卡 | 不新建 workspace，把已有的选中 |
| 三个并发调用 | 只产生 1 个组（`--idempotency-key` 生效），成员数正确 |
| worktree 已存在但 workspace 没开 | 直接开，不重建 worktree |

## 已知限制

1. **看板侧没有记录 cmux 分组**：`workspace_group:N` 没存进 `card_workspaces`，
   所以「工作区总览」目前不按仓库分组展示，卡片上也不显示所属组。
2. **脚本依赖 `jq`**：`kanban-open-ws` / `kanban-close-ws` 用 `jq` 解析 `--json`；
   看板内部的 `ensure_cmux_group` 自己解析，不依赖 jq。
3. **纯中文标题的 slug 会退化成 `task`**：`{slug}` 只保留 ASCII 字母数字，
   所以「语音」→ `task`。目录名靠 `{card_id}` 前缀保证唯一（`8-task`）。

---

# 删除工作区（同时删掉 worktree）

## cmux 的隐藏行为（已实测）

**`cmux workspace close` 会顺手 `git worktree remove` 掉该 workspace 对应的 worktree。**

| 场景 | 结果 |
|---|---|
| worktree 干净 → `cmux workspace close <ref> --force` | workspace 关闭，**worktree 目录与 git 注册一起消失** |
| worktree 有未提交改动 → 同上 | workspace 关闭，worktree **保留**（git 拒绝删除） |

所以在 cmux 里"删工作区"和"删 worktree"是同一件事，不需要额外命令。
`cmux-wt` 的源码里也留了注释说明这点"help 里看不到，容易误以为数据丢了"。

反过来的坑更值得记：**想关窗口但保留 worktree，必须放一个未跟踪文件**（`cmux-wt` 用的是
`.cmux-wt-keep`），因为 git 只有在"有未跟踪文件"时才会拒绝删除。

## 三种正确做法

| 目的 | 做法 |
|---|---|
| 两个都删（干净） | `cmux workspace close <ref> --force` |
| 两个都删（有未提交改动） | 先 close，再 `git -C <repo> worktree remove --force <path>` |
| 只关窗口、保留目录 | 先在目录里 `touch .cmux-wt-keep`，再 close，然后删掉这个标记文件 |

**顺序不能反**：先 `git worktree remove` 会让 cmux 里留下一个指向已删目录的死窗口。

## 看板里的实现

`scripts/kanban-close-ws`（工作区总览里按 `d` 会调它）：

```
kanban-close-ws --repo {repo} --worktree {worktree} [--force]
```

1. 按 **归一化后的路径** 找出 cwd 在这个 worktree 下的所有 workspace（含子目录），逐个 close
2. 等它们从 `workspace list` 里消失（cmux 关闭是异步的）
3. 仍有关不掉的窗口 → **不删目录**，退出码 4，避免死窗口
4. 目录还在（脏的、或本来没开窗口）→ `git worktree remove`；脏且没加 `--force` → 退出码 3，保留目录
5. 成功 → 看板删除 `card_workspaces` 记录，提示"已关闭 workspace 并删除 worktree"

### 踩过的两个坑

1. **路径匹配**：macOS 上 `/tmp` 是 `/private/tmp` 的符号链接，cmux 报的 `current_directory`
   有时解析、有时不解析。脚本对两边都做 `/private/* → /*` 归一化再比较，否则匹配不上、
   窗口没关却把目录删了（会留下死窗口）。
2. **异步延迟**：刚创建的 workspace 可能还不在 `workspace list` 里；close 之后也不会立刻消失。
   两个方向都要轮询几百毫秒，否则"关不掉"和"已关掉"都会误判。

## 验收（实测）

| 场景 | 结果 |
|---|---|
| 干净 worktree + 开着窗口 | 窗口关闭、目录删除、无残留 |
| 脏 worktree + 开着窗口，不加 `--force` | 窗口关闭、目录保留、退出码 3 |
| 脏 worktree + `--force` | 两者都删 |
| 没开窗口的 worktree | 直接删目录 |
| 目录已不存在 | 幂等，退出码 0 |
| 看板里按 `d`（端到端） | worktree 消失、cmux 窗口消失、DB 记录清除、提示正确 |
