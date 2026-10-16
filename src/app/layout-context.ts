import { createContext } from "react";

/** 仅协调界面抽屉，不参与宿主事件和会话生命周期。 */
export const LayoutContext = createContext({ navigationOpen: false, closeNavigation: () => {}, toggleNavigation: () => {} });
