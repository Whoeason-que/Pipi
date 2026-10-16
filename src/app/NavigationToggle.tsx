import { useContext } from "react";
import { IconPanel } from "../icons";
import { LayoutContext } from "./layout-context";

export default function NavigationToggle() {
  const layout = useContext(LayoutContext);
  return <button type="button" className="icon-btn navigation-toggle" aria-label="切换导航"
    aria-controls="primary-navigation" onClick={layout.toggleNavigation}><IconPanel /></button>;
}
