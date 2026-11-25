import { Channel, invoke } from "@tauri-apps/api/core";
import { generateUUID } from "@/utils/uuid";
import type {
  ContentRef,
  ImageRef,
  SessionContentSearchResult,
  SessionIndexStatus,
  SessionMessage,
  SessionMeta,
  TranscriptChunk,
} from "@/types";

export interface DeleteSessionOptions {
  providerId: string;
  sessionId: string;
  sourcePath: string;
}

export interface DeleteSessionResult extends DeleteSessionOptions {
  success: boolean;
  error?: string;
}

export interface RenameSessionOptions {
  providerId: string;
  sessionId: string;
  sourcePath: string;
  title: string;
}

/** get_session_block_content 的返回：按字符分页取工具输出 / 参数 / 思考 / diff 全文 */
export interface BlockContent {
  text: string;
  /** 全文字符数 */
  totalLen: number;
  /** 本页之后还有内容 */
  truncated: boolean;
  /** 下一页的起始字符偏移 */
  nextOffset?: number;
}

export interface BlockContentPage {
  offset?: number;
  limit?: number;
}

export type KimiResumeKind = "original" | "new";
export type KimiResumeAction = "open" | "copy";
export type KimiResumeMode = "direct" | "route" | "stack";

export interface KimiResumeTarget {
  sessionId: string;
  sourcePath: string;
}

export interface KimiResumeModel {
  alias: string;
  model: string;
  provider: string;
  displayName?: string | null;
  isDefault: boolean;
  revision: string;
}

export interface KimiResumeOptions {
  models: KimiResumeModel[];
  mode: KimiResumeMode;
}

export type KimiResumeOutcome =
  | { kind: "opened" }
  | { kind: "command"; command: string };

export const sessionsApi = {
  async list(): Promise<SessionMeta[]> {
    return await invoke("list_sessions");
  },

  /** 后台同步正文索引；立即返回当前进度，之后的进度走 session-index-status 事件 */
  async syncContentIndex(): Promise<SessionIndexStatus> {
    return await invoke("sync_session_index");
  },

  /** 在消息正文里搜索（按会话聚合）；providerIds 为空时不按应用过滤 */
  async searchContent(
    query: string,
    providerIds?: string[],
    signal?: AbortSignal,
  ): Promise<SessionContentSearchResult> {
    if (signal?.aborted)
      throw new DOMException("Search cancelled", "AbortError");
    const requestId = generateUUID();
    const cancel = () => {
      void invoke("cancel_session_content_search", { requestId }).catch(
        (error) => console.warn("取消会话正文搜索失败", error),
      );
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const result = await invoke<SessionContentSearchResult>(
        "search_session_content",
        { query, providerIds, requestId },
      );
      if (signal?.aborted)
        throw new DOMException("Search cancelled", "AbortError");
      return result;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  },

  /** 删除正文索引文件（关闭「搜索消息正文」时） */
  async clearContentIndex(): Promise<boolean> {
    return await invoke("clear_session_index");
  },

  async getMessages(
    providerId: string,
    sourcePath: string,
  ): Promise<SessionMessage[]> {
    return await invoke("get_session_messages", { providerId, sourcePath });
  },

  /**
   * 分块读取会话（stream_session_messages）：后端先推 Header，再按批推 Messages，
   * 最后 Done；失败时先推 Error 再以同一文案 reject。
   */
  async streamMessages(
    providerId: string,
    sourcePath: string,
    onChunk: (chunk: TranscriptChunk) => void,
  ): Promise<void> {
    const channel = new Channel<TranscriptChunk>();
    channel.onmessage = onChunk;
    await invoke("stream_session_messages", {
      providerId,
      sourcePath,
      onChunk: channel,
    });
  },

  /** 按 ContentRef 取全文（引用原样回传，前端不解析）；按字符分页，每页最多 512K 字符 */
  async getBlockContent(
    providerId: string,
    sourcePath: string,
    contentRef: ContentRef,
    page: BlockContentPage = {},
  ): Promise<BlockContent> {
    return await invoke("get_session_block_content", {
      providerId,
      sourcePath,
      contentRef,
      offset: page.offset,
      limit: page.limit,
    });
  },

  /** 取图片原始字节（后端返回 tauri::ipc::Response），调用方按 mediaType 转 Blob；SVG 会被后端拒绝 */
  async getImage(
    providerId: string,
    sourcePath: string,
    image: ImageRef,
  ): Promise<ArrayBuffer> {
    return await invoke("get_session_image", {
      providerId,
      sourcePath,
      image,
    });
  },

  /** 在 Finder / 文件管理器中显示本地路径（绝对路径或 file://，决策 D3） */
  async revealPath(path: string): Promise<boolean> {
    return await invoke("reveal_session_path", { path });
  },

  /** 弹出保存对话框把会话存成 Markdown 文件；取消时返回 null，成功返回保存路径 */
  async exportMarkdown(
    defaultName: string,
    content: string,
  ): Promise<string | null> {
    return await invoke("export_session_markdown", { defaultName, content });
  },

  async delete(options: DeleteSessionOptions): Promise<boolean> {
    const { providerId, sessionId, sourcePath } = options;
    return await invoke("delete_session", {
      providerId,
      sessionId,
      sourcePath,
    });
  },

  async deleteMany(
    items: DeleteSessionOptions[],
  ): Promise<DeleteSessionResult[]> {
    return await invoke("delete_sessions", { items });
  },

  async rename(options: RenameSessionOptions): Promise<boolean> {
    const { providerId, sessionId, sourcePath, title } = options;
    return await invoke("rename_session", {
      providerId,
      sessionId,
      sourcePath,
      title,
    });
  },

  async getKimiResumeOptions(
    target: KimiResumeTarget,
  ): Promise<KimiResumeOptions> {
    return await invoke("get_kimi_session_resume_options", { target });
  },

  async continueKimiSession(
    target: KimiResumeTarget,
    selection: Pick<KimiResumeModel, "alias" | "revision">,
    kind: KimiResumeKind,
    action: KimiResumeAction,
  ): Promise<KimiResumeOutcome> {
    return await invoke("continue_kimi_session", {
      target,
      selection,
      kind,
      action,
    });
  },

  async launchTerminal(options: {
    command: string;
    cwd?: string | null;
    customConfig?: string | null;
  }): Promise<boolean> {
    const { command, cwd, customConfig } = options;
    return await invoke("launch_session_terminal", {
      command,
      cwd,
      customConfig,
    });
  },
};
