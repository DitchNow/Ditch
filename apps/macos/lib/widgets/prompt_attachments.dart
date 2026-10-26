import 'dart:io';
import 'package:flutter/material.dart';
import '../data/prompt_attachment.dart';
import '../design_system/ditch_theme.dart';

class PromptAttachments extends StatelessWidget {
  const PromptAttachments({
    required this.attachments,
    required this.onRemove,
    super.key,
  });
  final List<PromptAttachment> attachments;
  final ValueChanged<PromptAttachment>? onRemove;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: 98,
    child: ListView.separated(
      scrollDirection: Axis.horizontal,
      itemCount: attachments.length,
      separatorBuilder: (_, _) => const SizedBox(width: 8),
      itemBuilder: (context, index) {
        final attachment = attachments[index];
        const fallback = Icon(Icons.insert_drive_file_outlined, size: 32);
        return Tooltip(
          message: attachment.path,
          child: Semantics(
            label: 'Included file ${attachment.filename}',
            child: Container(
              width: 128,
              padding: const EdgeInsets.all(4),
              decoration: BoxDecoration(
                color: context.ditch.surface,
                borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
                border: Border.all(color: context.ditch.separator),
              ),
              child: Column(
                children: [
                  Expanded(
                    child: Stack(
                      children: [
                        Positioned.fill(
                          child:
                              attachment.previewKind ==
                                  AttachmentPreviewKind.image
                              ? Image.file(
                                  File(attachment.path),
                                  fit: BoxFit.contain,
                                  cacheWidth: 256,
                                  cacheHeight: 128,
                                  errorBuilder: (_, _, _) => fallback,
                                )
                              : fallback,
                        ),
                        Positioned(
                          right: 0,
                          top: 0,
                          child: IconButton(
                            tooltip: 'Remove ${attachment.filename}',
                            style: IconButton.styleFrom(
                              backgroundColor: context.ditch.surface,
                            ),
                            constraints: const BoxConstraints.tightFor(
                              width: 24,
                              height: 24,
                            ),
                            padding: EdgeInsets.zero,
                            iconSize: 16,
                            onPressed: onRemove == null
                                ? null
                                : () => onRemove!(attachment),
                            icon: const Icon(Icons.close),
                          ),
                        ),
                      ],
                    ),
                  ),
                  Text(
                    attachment.filename,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ],
              ),
            ),
          ),
        );
      },
    ),
  );
}
