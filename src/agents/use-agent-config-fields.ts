//! Agent 配置表单的共享状态与联动。
//! 两个调用方的页面结构刻意不同（工作台是设置页：标签页 + 测试面板 + 保存/校验；
//! 新建表单是模态），所以这里只共享状态与「换供应商清模型」等联动，
//! JSX、保存逻辑与工作量专属字段（subagent / 压缩阈值 / promptScope / 已保存限额）
//! 仍由各自组件拥有。
import { useCallback, useState } from "react";
import type { CatalogModel } from "../catalog";
import type { BashMode, SandboxMode } from "../types";

export interface AgentConfigFieldsInit {
  /** 当前选中的供应商 id（空串 = 未绑定） */
  providerId: string;
  modelId: string;
  description: string;
  workspace: string;
  tools: string[];
  bashMode: BashMode;
  commands: string;
  sandbox: SandboxMode;
}

export function useAgentConfigFields(init: AgentConfigFieldsInit) {
  const [providerId, setProviderId] = useState(init.providerId);
  const [modelId, setModelId] = useState(init.modelId);
  // 目录里选的模型：用它回填 maxTokens / contextWindow（手填的模型保持调用方给定的原值）
  const [pickedModel, setPickedModel] = useState<CatalogModel | undefined>(undefined);
  const [description, setDescription] = useState(init.description);
  const [workspace, setWorkspace] = useState(init.workspace);
  const [tools, setTools] = useState(init.tools);
  const [bashMode, setBashMode] = useState(init.bashMode);
  const [commands, setCommands] = useState(init.commands);
  const [sandbox, setSandbox] = useState(init.sandbox);

  // 切换供应商必须清掉上一个供应商残留的模型 ID 与目录选择，
  // 否则会保存出「B 的端点 + A 的模型」这种静默错配。
  const changeProvider = useCallback((nextProviderId: string) => {
    setProviderId(nextProviderId);
    setModelId("");
    setPickedModel(undefined);
  }, []);

  // 手填（model 为 undefined）时由调用方保留已有限额：不能因为改了个 ID 就静默把
  // 目录/用户先前的 maxTokens、contextWindow 复位成默认值。
  const changeModel = useCallback((nextModelId: string, model?: CatalogModel) => {
    setModelId(nextModelId);
    setPickedModel(model);
  }, []);

  const toggleTool = useCallback((tool: string) => {
    setTools((previous) =>
      previous.includes(tool) ? previous.filter((candidate) => candidate !== tool) : [...previous, tool],
    );
  }, []);

  return {
    providerId,
    modelId,
    pickedModel,
    description,
    workspace,
    tools,
    bashMode,
    commands,
    sandbox,
    setDescription,
    setWorkspace,
    setTools,
    setBashMode,
    setCommands,
    setSandbox,
    changeProvider,
    changeModel,
    toggleTool,
  };
}
