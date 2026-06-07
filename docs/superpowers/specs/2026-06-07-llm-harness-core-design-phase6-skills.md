### 5.5 Skills 与 PromptTemplates

> **Skills 和 PromptTemplates 是什么？** Skills 是框架加载的 Markdown 文件——包含特定任务的详细指令（如 "如何写 commit message"、"如何部署到 K8s"）。它们被注入到 LLM 的 system prompt 中，LLM 可以自主决定何时调用哪个 skill。PromptTemplates 是参数化的 prompt 模板——调用方可以通过名称 + 参数快速构造标准化的 prompt。

---

#### Skills

```rust
pub struct Skill {
    pub name:                     String,   // 校验：小写字母+数字+连字符，≤64
    pub label:                    Option<String>,
    pub description:              String,   // 校验：非空，≤1024
    pub content:                  String,
    pub source:                   PathBuf,
    pub disable_model_invocation: bool,     // true: 不进 system prompt，仅供显式调用
}

pub struct SkillDiagnostic { pub source: PathBuf, pub level: DiagnosticLevel, pub message: String }
// DiagnosticLevel 定义见 §3.1

pub struct SourcedSkill {
    pub skill:  Skill,
    pub source_tag: String,  // 调用方提供的来源标记（如 "user-config", "project-local"）
}
```

> **设计理由：**
>
> **`name` 的校验规则（小写字母+数字+连字符，≤64）：** 名称是 LLM 在 system prompt 中看到的标识符——它需要简洁、机器友好、无歧义。小写+连字符避免了大小写混淆和空格转义问题。≤64 字符确保名称不会占用太多 system prompt token。
>
> **`label` vs `name`：** `label` 是可选的 UI 友好显示名。如果不存在，UI 回退到 `name`。例如 name = `"k8s-deploy"`，label = `"Kubernetes Deploy"`。
>
> **`description` 校验（非空，≤1024）：** 描述是 LLM 判断 "是否该用这个 skill" 的依据——必须存在且简洁。空描述意味着 LLM 永远不知道该 skill 何时适用——等同于禁用的 skill。1024 字符限制防止过长的描述占用 system prompt token。
>
> **`content`：** skill 文件的完整正文（YAML frontmatter 之后的部分）。这是 LLM 在调用 skill 时收到的详细指令。不做长度校验——skill 的内容由 skill 作者控制。
>
> **`source`：** skill 文件的绝对路径。用于：(1) LLM 被告知 skill 的位置（用于解析相对路径引用）；(2) 调试——知道 skill 来自哪个文件；(3) 重新加载——路径不变的 skill 可以跳过重解析。
>
> **`disable_model_invocation`：** 某些 skill 不应该由 LLM 自主选择——它们只能通过 `harness.skill(name)` 显式调用。例如 "系统管理" skill（可以修改配置、重启服务）——你不希望 LLM 在对话中自主触发它。将此字段设为 `true` 会将 skill 从 system prompt 的 available_skills 列表排除。
>
> **`SourcedSkill`：** 包装了 `Skill` + `source_tag`。"来源标记" 标识 skill 的来源——是用户配置（`"user-config"`）、项目本地（`"project-local"`）、还是插件（`"plugin:xxx"`）。框架不解释 `source_tag`——应用层自行定义语义。这支持 "显示所有 skill，按来源分组" 的 UI。

---

```rust
/// 递归扫描目录；每目录只取第一个 SKILL.md（不递归子目录的 SKILL.md）。
/// 校验 name 匹配父目录名。遵守 .gitignore / .ignore / .fdignore。
/// 解析符号链接。
pub async fn load_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<Skill>, Vec<SkillDiagnostic>);

pub async fn load_sourced_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[(String, PathBuf)],  // (source_tag, dir)
) -> (Vec<SourcedSkill>, Vec<SkillDiagnostic>);
```

> **加载算法的设计理由：**
>
> **"每目录只取第一个 SKILL.md"：** Skill 目录的结构是 `skill-name/SKILL.md`。`SKILL.md` 所在的目录名就是 skill name（除非 frontmatter 中显式指定了 name）。如果子目录中也有 SKILL.md，那是另一个独立的 skill（不是当前 skill 的子模块）。不递归子目录的 SKILL.md 避免了意外的 skill 嵌套。
>
> **名称匹配父目录名：** 强制一致性——如果目录叫 `k8s-deploy/`，skill name 必须是 `k8s-deploy`。这避免了 frontmatter 中的 name 与目录名不同步导致的困惑。如果确实需要不同的 name，在 frontmatter 中显式指定（会产生 warning 但允许通过）。
>
> **遵守 `.gitignore` / `.ignore` / `.fdignore`：** 如果 skill 目录或其父目录被 ignore 规则覆盖，该 skill 不会被加载。这对于 monorepo 场景很重要——`node_modules/` 中的 skill 文件不会被意外加载。
>
> **解析符号链接：** 如果 skill 文件或目录是符号链接，跟随它并解析目标。这允许通过符号链接组织 skill 集合（如 `skills/active → skills/v2`）。
>
> **返回 `(Vec<Skill>, Vec<SkillDiagnostic>)` 而非 `Result`：** 加载是 "最佳努力"——单个 skill 文件的解析失败不应导致整个加载失败。调用方仍然得到所有成功加载的 skill + 所有 diagnostic（哪些文件失败了、为什么）。

---

```rust
/// 注入 system prompt（仅 disable_model_invocation=false 的 skill）
pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String;

/// 显式调用：将 skill 内容包装为 `<skill name="...">...</skill>` 块，作为 user 消息注入
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String;
```

> **两种使用方式的区别：**
> - `format_skills_for_system_prompt`：生成一段 XML 格式的 skill 列表（name + description + location），注入到 system prompt 中。LLM 看到的是 "我有这些 skill 可用"。LLM 自主决定何时使用哪个 skill。
> - `format_skill_invocation`：将单个 skill 的完整内容包装为 `<skill>` 块，作为 user 消息发送。这是显式调用——调用方通过 `harness.skill("k8s-deploy")` 强制 LLM 使用特定 skill。
>
> **显式调用时 skill 内容的包装格式：** `<skill name="k8s-deploy" location="/path/to/skill">...content...</skill>`。LLM 被指示 "按照 skill 中的指令执行"。`additional_instructions` 追加在 skill 块之后——允许调用方在 skill 基础上添加额外约束（如 "只部署 staging，不要碰 production"）。

---

#### PromptTemplates

**位置参数 + shell-style 引号解析**，对齐 pi-agent-core：占位符 `$1`、`$2`、`$@`、`$ARGUMENTS`、`${@:N}`、`${@:N:L}`。

```rust
pub struct PromptTemplate {
    pub name:    String,
    pub content: String,
    pub source:  PathBuf,
}

pub async fn load_prompt_templates(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<PromptTemplate>, Vec<SkillDiagnostic>);

/// args 为位置参数列表；invoke 内部 shell-style 解析输入
pub fn invoke_template(
    template: &PromptTemplate,
    args:     &[String],
) -> Result<String, TemplateError>;
```

> **为什么是位置参数（`$1`, `$2`）而非命名参数（`{{name}}`）？** 对齐 pi-agent-core 的现有模板生态。位置参数配合 shell-style 引号解析允许模板像命令行工具一样使用：
> ```
> /deploy staging "update API endpoint" --dry-run
> → args = ["staging", "update API endpoint", "--dry-run"]
> → template: "Deploy to $1 environment. Task: $2. Flags: ${@:3}"
> → result: "Deploy to staging environment. Task: update API endpoint. Flags: --dry-run"
> ```
>
> **占位符语法：**
> - `$1`、`$2`...：第 N 个位置参数（1-based）
> - `$@` 或 `$ARGUMENTS`：所有参数以空格连接
> - `${@:N}`：从第 N 个参数开始的所有参数
> - `${@:N:L}`：从第 N 个参数开始的 L 个参数
>
> **shell-style 引号解析：** `args` 通常是用户输入的字符串拆分结果。双引号和单引号用于包裹含空格的参数。`invoke_template` 接收已经解析好的 `&[String]`——调用方负责解析引号。
>
> **为什么 `invoke_template` 是同步函数？** 参数替换是纯字符串操作——不涉及 I/O，不需要 async。

---

**测试策略：** Skills 和 Templates 的加载逻辑依赖 `dyn ExecutionEnv`。v1 **不**提供 `InMemoryEnv` mock——测试通过 `tempfile::TempDir` + `OsEnv` 真实运行：在临时目录布置 `SKILL.md` / 模板文件，调用 `load_skills()`，验证返回值。这样既覆盖了真实 fs 行为，又避免维护双重 env 实现。后续若需要 hermetic 测试再引入 `InMemoryEnv`。

> **为什么选择真实 fs 测试而非 mock？** mock `ExecutionEnv` 需要维护一个 "虚拟文件系统"——与真实文件系统的行为可能微妙不同（路径分隔符、符号链接解析、权限检查）。`tempfile` crate 提供隔离的临时目录，配合真实的 `OsEnv` 实现——测试运行在真实环境中，但互不干扰。代价是测试依赖文件系统（在 CI 中可用），但避免了 mock 维护成本。
