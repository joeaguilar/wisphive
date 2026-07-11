interface ParsedToolInputBase {
  raw: unknown;
  value: Record<string, unknown>;
  fields: CommonToolInput;
}

export interface CommonToolInput {
  filePath: string | null;
  content: string | null;
  pattern: string | null;
  path: string | null;
  fileType: string | null;
  glob: string | null;
  outputMode: string | null;
  prompt: string | null;
  source: string | null;
  lastAssistantMessage: string | null;
}

export interface BashInput {
  command: string | null;
  description: string | null;
}

export interface EditInput {
  oldString: string | null;
  newString: string | null;
}

interface AskUserQuestionOption {
  label: string;
  description: string | null;
}

interface AskUserQuestion {
  header: string | null;
  question: string;
  options: AskUserQuestionOption[];
}

export interface AskUserQuestionInput {
  questions: AskUserQuestion[];
}

export type ParsedToolInput =
  | (ParsedToolInputBase & { kind: "bash"; input: BashInput })
  | (ParsedToolInputBase & { kind: "edit"; input: EditInput })
  | (ParsedToolInputBase & {
      kind: "ask-user-question";
      input: AskUserQuestionInput | null;
    })
  | (ParsedToolInputBase & { kind: "other" });

interface RawAskUserQuestionOption {
  label: string;
  description?: string;
}

interface RawAskUserQuestion {
  header?: string;
  question: string;
  options?: RawAskUserQuestionOption[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function isQuestionOption(value: unknown): value is RawAskUserQuestionOption {
  return isRecord(value)
    && typeof value.label === "string"
    && (value.description === undefined || typeof value.description === "string");
}

function isQuestion(value: unknown): value is RawAskUserQuestion {
  return isRecord(value)
    && typeof value.question === "string"
    && (value.header === undefined || typeof value.header === "string")
    && (value.options === undefined
      || (Array.isArray(value.options) && value.options.every(isQuestionOption)));
}

function parseQuestions(value: unknown): AskUserQuestionInput | null {
  if (!Array.isArray(value) || value.length === 0 || !value.every(isQuestion)) {
    return null;
  }

  return {
    questions: value.map((question) => ({
      header: question.header ?? null,
      question: question.question,
      options: (question.options ?? []).map((option) => ({
        label: option.label,
        description: option.description ?? null,
      })),
    })),
  };
}

/**
 * Validate untrusted tool input once and expose only normalized fields to
 * component renderers. Invalid AskUserQuestion payloads stay distinguishable
 * from other tools so callers can render a stable fallback instead of mapping
 * unknown nested values.
 */
export function parseToolInput(toolName: string, raw: unknown): ParsedToolInput {
  const value = isRecord(raw) ? raw : {};
  const fields: CommonToolInput = {
    filePath: stringField(value.file_path),
    content: stringField(value.content),
    pattern: stringField(value.pattern),
    path: stringField(value.path),
    fileType: stringField(value.type),
    glob: stringField(value.glob),
    outputMode: stringField(value.output_mode),
    prompt: stringField(value.prompt),
    source: stringField(value.source),
    lastAssistantMessage: stringField(value.last_assistant_message),
  };
  const base: ParsedToolInputBase = { raw, value, fields };
  const name = toolName.toLowerCase().replace(/[^a-z]/g, "");

  if (name === "bash") {
    return {
      ...base,
      kind: "bash",
      input: {
        command: stringField(value.command),
        description: stringField(value.description),
      },
    };
  }

  if (name === "edit" || name === "multiedit") {
    return {
      ...base,
      kind: "edit",
      input: {
        oldString: stringField(value.old_string),
        newString: stringField(value.new_string),
      },
    };
  }

  if (name === "askuserquestion") {
    return {
      ...base,
      kind: "ask-user-question",
      input: parseQuestions(value.questions),
    };
  }

  return { ...base, kind: "other" };
}
