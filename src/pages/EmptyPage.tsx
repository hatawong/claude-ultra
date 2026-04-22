interface EmptyPageProps {
  title: string;
}

function EmptyPage({ title }: EmptyPageProps) {
  return (
    <div className="h-full flex items-center justify-center">
      <div className="text-center">
        <h2 className="text-lg font-medium text-gray-400 dark:text-gray-500">{title}</h2>
        <p className="text-sm text-gray-300 dark:text-gray-600 mt-2">Coming soon</p>
      </div>
    </div>
  );
}

export default EmptyPage;
