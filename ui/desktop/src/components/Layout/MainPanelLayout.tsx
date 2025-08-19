import React from 'react';

export const MainPanelLayout: React.FC<{
  children: React.ReactNode;
  removeTopPadding?: boolean;
  backgroundColor?: string;
  style?: React.CSSProperties;
}> = ({ children, removeTopPadding = false, backgroundColor = 'bg-background-default', style = {} }) => {
  return (
    <div className={`h-dvh`}>
      {/* Padding top matches the app toolbar drag area height - can be removed for full bleed */}
      <div
        className={`flex flex-col ${backgroundColor} flex-1 min-w-0 h-full min-h-0 ${removeTopPadding ? '' : 'pt-[32px]'}`}
        style={style}
      >
        {children}
      </div>
    </div>
  );
};
