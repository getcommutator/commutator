# ponytail: one-off scene: the commutator as a technical drawing with a quarter cut away
# (like the old engraving). Line art from Freestyle, tone only where it matters; the cut faces
# are mid-grey so the dither turns them into a hatch-like pattern.
# blender -b -P scheme.py  -> scheme.png (transparent)
import bpy, bmesh, math, os

HERE = os.path.dirname(os.path.abspath(__file__))
bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene
PARTS = []


def mat(name, grey):
    m = bpy.data.materials.new(name)
    m.diffuse_color = (grey, grey, grey, 1)
    return m


WHITE = mat("white", 1.0)
SHAFT = mat("shaft", 0.82)
MICA = mat("mica", 0.35)
CUT = mat("cut", 0.55)         # section faces: become a hatch-like dither


def link(bm, m):
    me = bpy.data.meshes.new("p")
    bm.to_mesh(me)
    ob = bpy.data.objects.new("p", me)
    sc.collection.objects.link(ob)
    ob.data.materials.append(m)
    PARTS.append(ob)
    return ob


def sector(a0, a1, r0, r1, z0, z1, m, steps=None):
    steps = steps or max(2, int(abs(a1 - a0) / 0.05))
    bm = bmesh.new()
    pts = [(r1, a0 + (a1 - a0) * i / steps) for i in range(steps + 1)] + \
          [(r0, a1 - (a1 - a0) * i / steps) for i in range(steps + 1)]
    bm.faces.new([bm.verts.new((r * math.cos(a), r * math.sin(a), z0)) for r, a in pts])
    ext = bmesh.ops.extrude_face_region(bm, geom=bm.faces[:])
    bmesh.ops.translate(bm, vec=(0, 0, z1 - z0), verts=[v for v in ext["geom"] if isinstance(v, bmesh.types.BMVert)])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    return link(bm, m)


def cap(a, r0, r1, z0, z1, toward):
    # a section face lying in the cut plane at angle a, nudged into the opening
    e = 0.002 * toward
    ca, sa = math.cos(a), math.sin(a)
    nx, ny = -sa * e, ca * e
    bm = bmesh.new()
    bm.faces.new([bm.verts.new((r * ca + nx, r * sa + ny, z)) for r, z in ((r0, z0), (r1, z0), (r1, z1), (r0, z1))])
    return link(bm, CUT)


# kept arc; the lower-front quarter is cut away, like the engraving (local 180° = up, 270° = toward camera)
A0, A1 = math.radians(0), math.radians(270)
# few wide bars on a short drum (24 thin bars on a long one read as noise at icon size)
N, L = 12, 0.55                                  # bars, drum length
step = 2 * math.pi / N
# (r0, r1, z0, z1, material) for the continuous parts; bars/risers are separate segments
rings = [(0.16, 0.34, 0.08, L, MICA),          # core / sleeve
         (0.24, 0.42, 0.0, 0.1, WHITE)]          # front clamp ring, smaller than the bars so they show
for r0, r1, z0, z1, m in rings:
    sector(A0, A1, r0, r1, z0, z1, m)
for i in range(N):
    a = i * step
    mid = (a + step / 2 - A0) % (2 * math.pi)
    if mid > A1 - A0:
        continue                                 # inside the cut quarter
    sector(a + 0.025, a + step - 0.025, 0.34, 0.5, 0.1, L, WHITE)          # bars
    sector(a + 0.07, a + step - 0.07, 0.5, 0.68, L - 0.14, L, WHITE)       # risers
for a, toward in ((A0, 1), (A1, -1)):
    for r0, r1, z0, z1 in ((0.16, 0.34, 0.08, L), (0.24, 0.42, 0.0, 0.1), (0.34, 0.5, 0.1, L),
                           (0.5, 0.68, L - 0.14, L)):
        cap(a, r0, r1, z0, z1, toward)
bpy.ops.mesh.primitive_cylinder_add(vertices=96, radius=0.14, depth=L + 0.3, location=(0, 0, L / 2))
bpy.context.object.data.materials.append(SHAFT)
PARTS.append(bpy.context.object)

bpy.ops.object.empty_add(rotation=(0, math.radians(90), 0))
pivot = bpy.context.object
for ob in PARTS:
    ob.parent = pivot

bpy.ops.object.camera_add(location=(-1.9, -4.0, 1.7))
cam = bpy.context.object
cam.data.lens = 100
bpy.ops.object.empty_add(location=(0.3, 0, 0.0))
t = cam.constraints.new("TRACK_TO")
t.target, t.track_axis, t.up_axis = bpy.context.object, "TRACK_NEGATIVE_Z", "UP_Y"
sc.camera = cam

sc.render.engine = "BLENDER_WORKBENCH"
sh = sc.display.shading
sh.light, sh.color_type = "FLAT", "MATERIAL"     # a drawing: white parts, tone only on sections
sh.show_cavity, sh.cavity_type = False, "WORLD"
sh.show_object_outline = True
sh.object_outline_color = (0, 0, 0)

sc.render.use_freestyle = True
sc.render.line_thickness_mode = "ABSOLUTE"
sc.render.line_thickness = 5.5
fs = sc.view_layers[0].freestyle_settings
fs.crease_angle = math.radians(140)
lineset = fs.linesets[0] if len(fs.linesets) else fs.linesets.new("ink")
lineset.select_silhouette = lineset.select_crease = lineset.select_border = True
lineset.select_material_boundary = True
if lineset.linestyle is None:
    lineset.linestyle = bpy.data.linestyles.new("ink")
lineset.linestyle.color = (0, 0, 0)

sc.render.film_transparent = True
sc.render.resolution_x = sc.render.resolution_y = 1024
sc.view_settings.view_transform = "Standard"
sc.render.filepath = os.path.join(HERE, "scheme.png")
bpy.ops.render.render(write_still=True)
