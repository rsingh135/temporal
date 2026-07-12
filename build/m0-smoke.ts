// M0 smoke: exercise the Fable-generated TypeScript domain.
import {
    WindowGeometry,
    WorkspaceId,
    geometryArea,
    workspaceIdValue,
} from "../ui/src/gen/domain/Types.ts";

const id = new WorkspaceId("m0-smoke");
const geometry = new WindowGeometry(100, 200, 1280, 800);
console.log(`ui M0 smoke: workspace=${workspaceIdValue(id)} area=${geometryArea(geometry)}`);
