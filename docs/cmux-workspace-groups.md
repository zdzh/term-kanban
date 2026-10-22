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

## 接到看板里

在分组设置（`g`）里把「工作区」按钮的打开命令换成这个脚本：

```
/Users/pikaka/work/task-kanban/scripts/kanban-open-ws \
  --repo {repo} --worktree {worktree} --branch {branch} --title {title}
```

> `{title}` 在这里是**参数值**而不是拼进 shell 的裸字符串——脚本自己按参数解析，
> 所以标题里有空格、引号都不会出问题。看板的模板层仍然禁止 `{title}` 出现在命令里，
> 这一条需要放宽成"作为独立参数传入"（见下文待办）。

想换 codex / vscode 仍然只是改这一行里的命令，脚本本身与工具无关。

## 验收（已实测）

| 场景 | 结果 |
|---|---|
| 首次打开某个 worktree | 建 worktree + 建组 + 建 workspace 并归组，锚点换成真实成员，空锚点被关闭 |
| 同一仓库打开第二张卡 | 复用同一个组（成员数 2，仍是 1 个组） |
| 重复点同一张卡 | 不新建 workspace，把已有的选中 |
| 三个并发调用 | 只产生 1 个组（`--idempotency-key` 生效），成员数正确 |
| worktree 已存在但 workspace 没开 | 直接开，不重建 worktree |

## 待办

1. **模板层放宽 `{title}` 的用法**：目前 `render_command` 遇到 `{title}` 直接报错。
   需要区分"拼进 shell 字符串"（危险，禁止）与"作为独立参数传给脚本"（安全）。
   最小改法：新增 `{title_arg}` 变量，渲染成已做单引号转义的形式，允许出现在命令里；
   `{title}` 仍保持禁止，避免误用。
2. **看板侧记录 cmux 分组**：把 `workspace_group:N` 存进 `card_workspaces`，
   这样「工作区总览」能按仓库分组展示，也能在卡片上显示它属于哪个组。
3. **脚本的 jq 依赖**：目前用 `jq` 解析 `--json`。若不想依赖，可换成 `python3 -c`（机器上都有）。
