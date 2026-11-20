FeatureScript 3112;
import(path : "onshape/std/geometry.fs", version : "3112.0");

export enum FramePart
{
    annotation { "Name" : "All parts, assembled" }
    ASSEMBLED,
    annotation { "Name" : "Front housing only" }
    FRONT,
    annotation { "Name" : "Rear cover only" }
    REAR,
    annotation { "Name" : "Button plunger only" }
    PLUNGER
}

function frameCuboid(context is Context, id is Id, low is Vector, high is Vector) returns Query
{
    fCuboid(context, id, { "corner1" : low, "corner2" : high });
    return qCreatedBy(id, EntityType.BODY);
}

function cylinder(context is Context, id is Id, x, y, low, high, radius) returns Query
{
    fCylinder(context, id, {
                "bottomCenter" : vector(x, y, low),
                "topCenter" : vector(x, y, high),
                "radius" : radius
            });
    return qCreatedBy(id, EntityType.BODY);
}

function frameXCylinder(context is Context, id is Id, low, high, y, z, radius) returns Query
{
    fCylinder(context, id, {
                "bottomCenter" : vector(low, y, z),
                "topCenter" : vector(high, y, z),
                "radius" : radius
            });
    return qCreatedBy(id, EntityType.BODY);
}

function subtract(context is Context, id is Id, target is Query, tools is array)
{
    opBoolean(context, id, {
                "targets" : target,
                "tools" : qUnion(tools),
                "operationType" : BooleanOperationType.SUBTRACTION
            });
}

function join(context is Context, id is Id, bodies is array)
{
    opBoolean(context, id, {
                "tools" : qUnion(bodies),
                "operationType" : BooleanOperationType.UNION
            });
}

annotation { "Feature Type Name" : "Hall of Fame enclosure" }
export const hofFrame = defineFeature(function(context is Context, id is Id, definition is map)
    precondition
    {
        annotation { "Name" : "Parts", "Default" : FramePart.ASSEMBLED }
        definition.part is FramePart;

        annotation { "Name" : "Clear electronics depth" }
        isLength(definition.electronicsDepth,
            { (millimeter) : [20, 22, 40] } as LengthBoundSpec);

        annotation { "Name" : "Minimum border outside panel" }
        isLength(definition.border,
            { (millimeter) : [12, 12, 25] } as LengthBoundSpec);

        annotation { "Name" : "Inner front round-over radius (0 = flat)" }
        isLength(definition.frontRadius,
            { (millimeter) : [0, 4, 5] } as LengthBoundSpec);

        annotation { "Name" : "Panel clearance per side" }
        isLength(definition.panelClearance,
            { (millimeter) : [0.2, 0.3, 0.35] } as LengthBoundSpec);

        annotation { "Name" : "M3 pilot diameter (test first)" }
        isLength(definition.pilotDiameter,
            { (millimeter) : [2.3, 2.5, 2.9] } as LengthBoundSpec);

        annotation { "Name" : "Button from portrait panel left" }
        isLength(definition.buttonFromLeft,
            { (millimeter) : [35, 54, 96] } as LengthBoundSpec);

        annotation { "Name" : "Button from portrait panel bottom" }
        isLength(definition.buttonFromBottom,
            { (millimeter) : [15, 63, 155] } as LengthBoundSpec);

        annotation { "Name" : "Button center behind glass rear" }
        isLength(definition.buttonDepth,
            { (millimeter) : [5, 10, 14] } as LengthBoundSpec);

        annotation { "Name" : "Plunger resting gap to button" }
        isLength(definition.buttonGap,
            { (millimeter) : [0, 0.3, 1] } as LengthBoundSpec);

        annotation { "Name" : "Plunger maximum travel (check switch)" }
        isLength(definition.plungerTravel,
            { (millimeter) : [0.2, 0.6, 1.2] } as LengthBoundSpec);

        annotation { "Name" : "Button center above PCB component face" }
        isLength(definition.pcbButtonHeight,
            { (millimeter) : [3, 3.5, 4] } as LengthBoundSpec);

        annotation { "Name" : "Button projection beyond PCB left edge" }
        isLength(definition.pcbButtonProjection,
            { (millimeter) : [0, 1, 2] } as LengthBoundSpec);

        annotation { "Name" : "Button center from PCB bottom" }
        isLength(definition.pcbButtonFromBottom,
            { (millimeter) : [25, 32, 40] } as LengthBoundSpec);
    }
    {
        const panelWidth = 111.20 * millimeter;
        const panelHeight = 170.20 * millimeter;
        const panelThickness = 1.25 * millimeter;
        const windowWidth = 98.83 * millimeter;
        const windowHeight = 164.22 * millimeter;
        // Rotate the supplied drawing counterclockwise: flex on the right, image offset left.
        const windowX = -panelWidth / 2 + 2.98 * millimeter + windowWidth / 2;
        const windowY = 0 * millimeter;
        const frontThickness = 2.4 * millimeter;
        const frontZ = -definition.frontRadius;
        const wall = 2.4 * millimeter;
        const coverThickness = 3 * millimeter;
        const frontFoam = 0.5 * millimeter;
        const rearFoam = 0.5 * millimeter;
        const panelBack = frontThickness + frontFoam + panelThickness;
        const railTip = panelBack + rearFoam;
        const seam = railTip + definition.electronicsDepth;
        const back = seam + coverThickness;
        // Center the outside on the offset window and keep all four visible borders equal.
        // The right side of the panel is farthest from the window center.
        const width = 2 * (panelWidth / 2 - windowX + definition.border);
        const height = windowHeight + width - windowWidth;
        const left = windowX - width / 2;
        const right = windowX + width / 2;
        const bottom = windowY - height / 2;
        const top = windowY + height / 2;
        const pocketWidth = panelWidth + 2 * definition.panelClearance;
        const pocketHeight = panelHeight + 2 * definition.panelClearance;
        const locatorWall = 1.6 * millimeter;
        const overlap = 0.1 * millimeter;
        const overshoot = 1 * millimeter;
        const postHalf = 4 * millimeter;
        const screwPositions = [
            vector(left + 5 * millimeter, bottom + 5 * millimeter),
            vector(right - 5 * millimeter, bottom + 5 * millimeter),
            vector(left + 5 * millimeter, top - 5 * millimeter),
            vector(right - 5 * millimeter, top - 5 * millimeter)
        ];
        const buttonX = -panelWidth / 2 + definition.buttonFromLeft;
        const buttonY = -panelHeight / 2 + definition.buttonFromBottom;
        const buttonZ = panelBack + definition.buttonDepth;
        const shaftRadius = 3 * millimeter;
        const guideRadius = shaftRadius + 0.3 * millimeter;
        const collarRadius = 5 * millimeter;
        const innerLeft = left + wall;
        const collarEnd = innerLeft + 2 * millimeter;
        const stopX = collarEnd + definition.plungerTravel;
        const fingerX = left - 2 * millimeter;
        const tipX = buttonX - definition.buttonGap;
        const tipShoulderX = tipX - 3 * millimeter;
        const guidePositions = [
            vector(stopX, stopX + 4 * millimeter),
            vector(buttonX - 10 * millimeter, buttonX - 6 * millimeter)
        ];
        const pcbWidth = 41.232385 * millimeter;
        const pcbHeight = 80 * millimeter;
        const pcbThickness = 1.6 * millimeter;
        const pcbLeft = buttonX + definition.pcbButtonProjection;
        const pcbBottom = buttonY - definition.pcbButtonFromBottom;
        const pcbComponentFace = buttonZ - definition.pcbButtonHeight;
        const pcbScreenFace = pcbComponentFace - pcbThickness;
        // EE04 V1.2 Edge.Cuts holes, referenced to the installed board's left/bottom edges.
        const pcbMountPositions = [
            vector(pcbLeft + 2.473707 * millimeter, pcbBottom + 2.553309 * millimeter),
            vector(pcbLeft + 38.679074 * millimeter, pcbBottom + 2.553309 * millimeter),
            vector(pcbLeft + 2.473707 * millimeter, pcbBottom + 77.456640 * millimeter),
            vector(pcbLeft + 38.679074 * millimeter, pcbBottom + 77.456640 * millimeter)
        ];

        if (definition.plungerTravel <= definition.buttonGap)
        {
            throw regenError("Plunger travel must exceed the resting gap to reach the button.",
                ["plungerTravel", "buttonGap"]);
        }
        if (pcbScreenFace - 2 * millimeter <= panelBack)
        {
            throw regenError("PCB or screw heads are too close to the glass. Increase button depth or reduce its height above the PCB.",
                ["buttonDepth", "pcbButtonHeight"]);
        }
        if (pcbLeft < left + wall + 0.5 * millimeter ||
            pcbLeft + pcbWidth > right - wall - 0.5 * millimeter)
        {
            throw regenError("The PCB would hit a side wall. Adjust the button's horizontal position or PCB projection.",
                ["buttonFromLeft", "pcbButtonProjection"]);
        }
        if (pcbBottom < -panelHeight / 2 + 1.7 * millimeter ||
            pcbBottom + pcbHeight > panelHeight / 2 - 1.7 * millimeter)
        {
            throw regenError("The PCB would hit a display-support rail. Adjust the button's vertical position or its offset from PCB bottom.",
                ["buttonFromBottom", "pcbButtonFromBottom"]);
        }

        var frameBodies = [];
        if (definition.part == FramePart.ASSEMBLED || definition.part == FramePart.FRONT)
        {
            const housing = frameCuboid(context, id + "housing",
                vector(left, bottom, frontZ),
                vector(right, top, seam));
            const cavity = frameCuboid(context, id + "cavity",
                vector(left + wall, bottom + wall, frontThickness),
                vector(right - wall, top - wall, seam + overshoot));
            subtract(context, id + "hollow", housing, [cavity]);
            const window = frameCuboid(context, id + "window",
                vector(windowX - windowWidth / 2, windowY - windowHeight / 2, frontZ - overshoot),
                vector(windowX + windowWidth / 2, windowY + windowHeight / 2, frontThickness + overlap));
            subtract(context, id + "openWindow", housing, [window]);

            if (definition.frontRadius > 0 * millimeter)
            {
                const frontEdges = qCoincidesWithPlane(
                    qOwnedByBody(housing, EntityType.EDGE),
                    plane(vector(0 * millimeter, 0 * millimeter, frontZ), vector(0, 0, 1)));
                const innerEdges = qUnion([
                    qContainsPoint(frontEdges, vector(windowX - windowWidth / 2, windowY, frontZ)),
                    qContainsPoint(frontEdges, vector(windowX + windowWidth / 2, windowY, frontZ)),
                    qContainsPoint(frontEdges, vector(windowX, windowY - windowHeight / 2, frontZ)),
                    qContainsPoint(frontEdges, vector(windowX, windowY + windowHeight / 2, frontZ))
                ]);
                opFillet(context, id + "roundFront", {
                            "entities" : innerEdges,
                            "radius" : definition.frontRadius
                        });
            }

            const locator = frameCuboid(context, id + "locator",
                vector(-pocketWidth / 2 - locatorWall, -pocketHeight / 2 - locatorWall,
                    frontThickness - overlap),
                vector(pocketWidth / 2 + locatorWall, pocketHeight / 2 + locatorWall, railTip));
            const pocket = frameCuboid(context, id + "panelPocket",
                vector(-pocketWidth / 2, -pocketHeight / 2, frontThickness - overshoot),
                vector(pocketWidth / 2, pocketHeight / 2, railTip + overshoot));
            // Keep the ribbon relief opposite the left-side button plunger.
            const flexRelief = frameCuboid(context, id + "flexRelief",
                vector(pocketWidth / 2 - overlap, -30 * millimeter,
                    frontThickness - overshoot),
                vector(pocketWidth / 2 + locatorWall + overshoot, 30 * millimeter, railTip + overshoot));
            subtract(context, id + "shapeLocator", locator, [pocket, flexRelief]);

            var housingBodies = [housing, locator];
            for (var i = 0; i < size(screwPositions); i += 1)
            {
                const position = screwPositions[i];
                const post = frameCuboid(context, id + ("post" ~ i),
                    vector(position[0] - postHalf, position[1] - postHalf, frontThickness - overlap),
                    vector(position[0] + postHalf, position[1] + postHalf, seam));
                housingBodies = append(housingBodies, post);
            }
            join(context, id + "joinHousing", housingBodies);

            var pilots = [];
            for (var i = 0; i < size(screwPositions); i += 1)
            {
                const position = screwPositions[i];
                pilots = append(pilots, cylinder(context, id + ("pilot" ~ i),
                    position[0], position[1], seam - 10 * millimeter,
                    seam + overshoot, definition.pilotDiameter / 2));
            }
            subtract(context, id + "drillPilots", housing, pilots);
            const buttonBore = frameXCylinder(context, id + "buttonBore",
                left - overshoot, innerLeft + overshoot,
                buttonY, buttonZ, guideRadius);
            subtract(context, id + "openButtonBore", housing, [buttonBore]);
            setProperty(context, {
                        "entities" : housing,
                        "propertyType" : PropertyType.NAME,
                        "value" : "Front housing"
                    });
            frameBodies = append(frameBodies, housing);
        }

        if (definition.part == FramePart.ASSEMBLED || definition.part == FramePart.REAR)
        {
            const cover = frameCuboid(context, id + "cover",
                vector(left, bottom, seam),
                vector(right, top, back));
            var coverBodies = [cover];
            // Retain the same glass borders after rotating the panel: now top and bottom.
            for (var i = 0; i < 2; i += 1)
            {
                const railMinY = i == 0 ?
                    -panelHeight / 2 - 2.3 * millimeter :
                    panelHeight / 2 - 1.2 * millimeter;
                const rail = frameCuboid(context, id + ("rail" ~ i),
                    vector(-panelWidth / 2 + 8 * millimeter, railMinY, railTip),
                    vector(panelWidth / 2 - 8 * millimeter, railMinY + 3.5 * millimeter,
                        seam + overlap));
                coverBodies = append(coverBodies, rail);
            }
            for (var i = 0; i < size(pcbMountPositions); i += 1)
            {
                const position = pcbMountPositions[i];
                coverBodies = append(coverBodies, cylinder(context, id + ("pcbPost" ~ i),
                    position[0], position[1], pcbComponentFace, seam + overlap, 2.4 * millimeter));
            }
            // Front-open forks let the cover lift off without threading the long pin through it.
            for (var i = 0; i < size(guidePositions); i += 1)
            {
                const position = guidePositions[i];
                const guide = frameCuboid(context, id + ("guide" ~ i),
                    vector(position[0], buttonY - 6 * millimeter, railTip),
                    vector(position[1], buttonY + 6 * millimeter, seam + overlap));
                coverBodies = append(coverBodies, guide);
            }
            join(context, id + "joinCover", coverBodies);

            var coverCuts = [];
            for (var i = 0; i < size(pcbMountPositions); i += 1)
            {
                const position = pcbMountPositions[i];
                coverCuts = append(coverCuts, cylinder(context, id + ("pcbPilot" ~ i),
                    position[0], position[1], pcbComponentFace - overshoot,
                    pcbComponentFace + 6 * millimeter, 0.8 * millimeter));
            }
            for (var i = 0; i < size(guidePositions); i += 1)
            {
                const position = guidePositions[i];
                coverCuts = append(coverCuts, frameCuboid(context, id + ("guideSlot" ~ i),
                    vector(position[0] - overlap, buttonY - guideRadius, railTip - overshoot),
                    vector(position[1] + overlap, buttonY + guideRadius, buttonZ + guideRadius)));
            }
            for (var i = 0; i < size(screwPositions); i += 1)
            {
                const position = screwPositions[i];
                coverCuts = append(coverCuts, cylinder(context, id + ("clearance" ~ i),
                    position[0], position[1], seam - overshoot, back + overshoot,
                    1.7 * millimeter));
                coverCuts = append(coverCuts, cylinder(context, id + ("headRecess" ~ i),
                    position[0], position[1], back - 1.8 * millimeter, back + overshoot,
                    3.2 * millimeter));
            }
            subtract(context, id + "drillCover", cover, coverCuts);
            setProperty(context, {
                        "entities" : cover,
                        "propertyType" : PropertyType.NAME,
                        "value" : "Rear cover - print exterior face down"
                    });
            frameBodies = append(frameBodies, cover);
        }

        if (definition.part == FramePart.ASSEMBLED || definition.part == FramePart.PLUNGER)
        {
            const shaft = frameXCylinder(context, id + "plungerShaft",
                fingerX, tipShoulderX, buttonY, buttonZ, shaftRadius);
            const tip = frameXCylinder(context, id + "plungerTip",
                tipShoulderX - overlap, tipX, buttonY, buttonZ, 1.5 * millimeter);
            const collar = frameXCylinder(context, id + "plungerCollar",
                innerLeft, collarEnd, buttonY, buttonZ, collarRadius);
            join(context, id + "joinPlunger", [shaft, tip, collar]);
            setProperty(context, {
                        "entities" : shaft,
                        "propertyType" : PropertyType.NAME,
                        "value" : "Side button plunger - separate moving part"
                    });
            frameBodies = append(frameBodies, shaft);
        }

        // Build with positive inward depth, then expose a front view with X right and Y up.
        opTransform(context, id + "orientFront", {
                    "bodies" : qUnion(frameBodies),
                    "transform" : mirrorAcross(
                        plane(vector(0, 0, 0) * millimeter, vector(0, 0, 1)))
                });
    });
