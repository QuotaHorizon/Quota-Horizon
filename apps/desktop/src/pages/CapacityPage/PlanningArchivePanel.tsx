import {getCapacityPlanningArchive} from "../../api/backend";
import {PlanningArchiveView} from "./PlanningArchiveView";

export function PlanningArchivePanel(props: Omit<Parameters<typeof PlanningArchiveView>[0], "read">) {
  return <PlanningArchiveView key={props.contextId} {...props} read={getCapacityPlanningArchive}/>;
}
