part of 'main.dart';

enum ConversationViewportMode { initializing, following, detached }

class ConversationViewportController extends ChangeNotifier {
  ConversationViewportController({ScrollController? scrollController})
    : scrollController = scrollController ?? _ConversationScrollController();

  static const nearLatestThreshold = 72.0;
  static const _arrivalDuration = Duration(milliseconds: 180);

  final ScrollController scrollController;
  ConversationViewportMode _mode = ConversationViewportMode.initializing;
  final Set<String> _knownItemIds = {};
  final Set<String> _knownMessageIds = {};
  List<ConversationItem> _items = const [];
  final Set<String> _unseenItemIds = {};
  bool _frameScheduled = false;
  bool _disposed = false;
  bool _userScrolling = false;
  VoidCallback? _onInitialPositioned;

  ConversationViewportMode get mode => _mode;
  int get unseenCount => _unseenItemIds.length;
  bool get isDetached => _mode == ConversationViewportMode.detached;

  // The transcript is reversed. All coordinate assumptions stay here:
  // visual latest = minScrollExtent; visual oldest = maxScrollExtent.
  double get _distanceFromLatest {
    if (!scrollController.hasClients) return 0;
    final position = scrollController.position;
    return position.pixels - position.minScrollExtent;
  }

  bool get isNearLatest =>
      !scrollController.hasClients ||
      _distanceFromLatest <= nearLatestThreshold;

  bool get isNearOldest {
    if (!scrollController.hasClients) return false;
    final position = scrollController.position;
    return position.maxScrollExtent - position.pixels <= nearLatestThreshold;
  }

  void beginOpening({VoidCallback? onInitialPositioned}) {
    _mode = ConversationViewportMode.initializing;
    _knownItemIds.clear();
    _knownMessageIds.clear();
    _items = const [];
    _unseenItemIds.clear();
    _onInitialPositioned = onInitialPositioned;
    _scheduleLatest();
    notifyListeners();
  }

  void attach({VoidCallback? onInitialPositioned}) {
    if (_mode == ConversationViewportMode.initializing) {
      _onInitialPositioned ??= onInitialPositioned;
      _scheduleLatest();
    } else if (_mode == ConversationViewportMode.following) {
      _scheduleLatest();
    }
  }

  void synchronizeItems(List<ConversationItem> items) {
    if (_disposed) return;
    final itemIds = items.map((item) => item.identity).toSet();
    final messages = [
      for (final item in items) ...[
        if (item.message != null) item.message!,
        ...item.toolMessages,
      ],
    ];
    final liveMessageIds = messages
        .where(
          (message) =>
              message.arrival == ChatMessageArrival.live &&
              !_knownMessageIds.contains(message.identity),
        )
        .map((message) => message.identity)
        .toSet();
    final added = items
        .where(
          (item) =>
              !_knownItemIds.contains(item.identity) &&
              (item.approval != null ||
                  liveMessageIds.contains(item.message?.identity) ||
                  item.toolMessages.any(
                    (message) => liveMessageIds.contains(message.identity),
                  )),
        )
        .map((item) => item.identity);
    final newArrivals = added.toList();
    _items = List.unmodifiable(items);
    _knownMessageIds
      ..clear()
      ..addAll(messages.map((message) => message.identity));
    _knownItemIds
      ..clear()
      ..addAll(itemIds);
    if (_mode == ConversationViewportMode.initializing) {
      _scheduleLatest();
      return;
    }
    if (isDetached) {
      final oldCount = unseenCount;
      // Retire removed rows (including a completed streaming placeholder).
      _unseenItemIds.retainAll(itemIds);
      _unseenItemIds.addAll(newArrivals);
      if (unseenCount != oldCount) notifyListeners();
    } else {
      _scheduleLatest();
    }
  }

  bool handleScrollNotification(ScrollNotification notification) {
    if (_disposed || !scrollController.hasClients) return false;
    if (notification is UserScrollNotification) {
      _userScrolling = notification.direction != ScrollDirection.idle;
    } else if (notification is ScrollStartNotification) {
      _userScrolling = notification.dragDetails != null || _userScrolling;
    }
    if (notification is ScrollUpdateNotification &&
        (_userScrolling || notification.dragDetails != null)) {
      final delta = notification.scrollDelta ?? 0;
      if (delta > 0 && !isDetached) {
        // In this reversed list a positive delta means reading older content.
        // Detach on the first movement, even inside the near-latest threshold.
        _mode = ConversationViewportMode.detached;
        notifyListeners();
      } else if (delta < 0 && isNearLatest) {
        _setFollowing();
      }
      return delta > 0;
    }
    if (notification is ScrollEndNotification) _userScrolling = false;
    return false; // Only actual movement toward history may paginate.
  }

  void showLatest() {
    if (_disposed) return;
    _mode = ConversationViewportMode.following;
    _userScrolling = false;
    _unseenItemIds.clear();
    notifyListeners();
    if (!scrollController.hasClients) {
      _scheduleLatest();
      return;
    }
    final position = scrollController.position;
    final distance = _distanceFromLatest;
    if (distance > position.viewportDimension * 4) {
      scrollController.jumpTo(position.minScrollExtent);
    } else {
      unawaited(
        scrollController.animateTo(
          position.minScrollExtent,
          duration: _arrivalDuration,
          curve: Curves.easeOut,
        ),
      );
    }
  }

  void _setFollowing() {
    if (_mode == ConversationViewportMode.following && _unseenItemIds.isEmpty) {
      return;
    }
    _mode = ConversationViewportMode.following;
    _unseenItemIds.clear();
    notifyListeners();
  }

  void _scheduleLatest() {
    if (_disposed || _frameScheduled) return;
    _frameScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _frameScheduled = false;
      if (_disposed) return;
      if (isDetached) return;
      if (!scrollController.hasClients) return;
      final position = scrollController.position;
      if (position.pixels != position.minScrollExtent) {
        scrollController.jumpTo(position.minScrollExtent);
      }
      if (_mode == ConversationViewportMode.initializing) {
        _mode = ConversationViewportMode.following;
        final callback = _onInitialPositioned;
        _onInitialPositioned = null;
        notifyListeners();
        callback?.call();
      }
    });
  }

  @override
  void dispose() {
    _disposed = true;
    _onInitialPositioned = null;
    scrollController.dispose();
    super.dispose();
  }
}

// Correct content displacement during layout, rather than jumpTo(), which
// cancels the active drag/ballistic activity and emits pagination notifications.
class _ConversationScrollController extends ScrollController {
  double? Function(double pixels)? anchorCorrection;

  @override
  ScrollPosition createScrollPosition(
    ScrollPhysics physics,
    ScrollContext context,
    ScrollPosition? oldPosition,
  ) => _ConversationScrollPosition(
    physics: physics,
    context: context,
    oldPosition: oldPosition,
    initialPixels: initialScrollOffset,
    keepScrollOffset: keepScrollOffset,
    correction: (pixels) => anchorCorrection?.call(pixels),
  );
}

class _ConversationScrollPosition extends ScrollPositionWithSingleContext {
  _ConversationScrollPosition({
    required super.physics,
    required super.context,
    super.oldPosition,
    super.initialPixels,
    super.keepScrollOffset,
    required this.correction,
  });

  final double? Function(double pixels) correction;

  @override
  bool applyContentDimensions(double minScrollExtent, double maxScrollExtent) {
    final delta = correction(pixels);
    if (delta != null && delta.abs() > 0.5) {
      final target = (pixels + delta).clamp(minScrollExtent, maxScrollExtent);
      if ((target - pixels).abs() > 0.5) {
        correctBy(target - pixels);
        return false;
      }
    }
    return super.applyContentDimensions(minScrollExtent, maxScrollExtent);
  }
}

class _ConversationAnchor {
  const _ConversationAnchor(this.key, this.y);
  final GlobalKey key;
  final double y;
}

class _ConversationAnchorMarker extends SingleChildRenderObjectWidget {
  const _ConversationAnchorMarker({required super.child, super.key});

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderConversationAnchorMarker();
}

class _RenderConversationAnchorMarker extends RenderProxyBox {
  double layoutHeight = 0;

  @override
  void performLayout() {
    super.performLayout();
    layoutHeight = size.height;
  }

  double? get viewportY {
    // The list adds same-size semantics/repaint wrappers around the row marker.
    // Read their layout offsets, not RenderBox.size during the viewport's layout.
    // Tool-entry markers also accumulate offsets inside their outer row marker.
    RenderObject node = this;
    var innerY = 0.0;
    var rowHeight = layoutHeight;
    while (node.parent != null) {
      final parent = node.parent!;
      if (parent is RenderSliverMultiBoxAdaptor && node is RenderBox) {
        if (parent.constraints.axisDirection != AxisDirection.up ||
            parent.geometry == null ||
            parent.geometry!.scrollOffsetCorrection != null) {
          return null;
        }
        final data = node.parentData;
        if (data is! SliverMultiBoxAdaptorParentData ||
            data.layoutOffset == null ||
            data.keptAlive) {
          return null;
        }
        final y =
            parent.geometry!.paintExtent -
            parent.childMainAxisPosition(node) -
            rowHeight +
            innerY;
        final viewport = RenderAbstractViewport.maybeOf(parent);
        if (viewport == null) return null;
        return MatrixUtils.transformPoint(
          parent.getTransformTo(viewport),
          Offset(0, y),
        ).dy;
      }
      final data = node.parentData;
      if (data is BoxParentData) innerY += data.offset.dy;
      node = parent;
      if (node is _RenderConversationAnchorMarker) {
        rowHeight = node.layoutHeight;
      }
    }
    return null;
  }
}
